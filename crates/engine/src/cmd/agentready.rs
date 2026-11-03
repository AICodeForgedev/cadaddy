//! The project as an agent with no memory and a small context needs it (`project.*`, `review.*`,
//! `bim.build`): state lives in the drawing, answers are compact and paged, repeating a call is
//! safe, refusals say what to call instead, and documents outrank an agent's code knowledge.
//!
//! - `project.briefing {}`: one page (a few thousand characters at most) — the revision, what is
//!   built per level and discipline, open review items by kind with the top few, recent changes,
//!   handoff notes and the suggested next call. A fresh agent resumes from it alone.
//! - `project.note {text, author?}`: a handoff note kept in the drawing.
//! - The review queue, ranked: the prints' gaps and conflicts (`rfi.list`), runs whose size or
//!   height is a guess (`mep.list`), clashes (`model.clashes`), and concerns agents raised.
//!   `review.list` (paged by cursor), `review.next`, `review.resolve` (an answer and, for anything
//!   the documents say, a citation of one: drawing, schedule, spec, approved submittal or RFI),
//!   `review.defer` (a field decision), `review.rfi_draft` (onto the RFI list), `review.batch` (one
//!   answer for many identical items: a preview and its confirm token first), `review.approve`
//!   and `review.log` (a person reviews what agents did).
//! - `bim.build {level?, discipline?}`: builds the model from the building spec and reports that
//!   level's elements; building twice gives the same model. With no spec yet it drafts one from
//!   the open print set (or the plan drawing) first.
//!
//! Everything lives in [`cadaddy_doc::Drawing::agent`] (notes, decisions with their citations and
//! authors, the change log, the revision) and is undoable like any edit. Review item ids are
//! stable: a hash of what the item is about, not its position in a list.

use std::collections::BTreeMap;

use cadaddy_doc::Drawing;
use cadaddy_walk3d::rfi::Rfi;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// The briefing's size at most, characters (about 1,500 tokens).
pub const BRIEFING_MAX: usize = 6_000;
/// Any other answer's size at most, characters.
pub const ANSWER_MAX: usize = 16_000;
const PAGE_DEFAULT: usize = 10;
const PAGE_MAX: usize = 50;
const NOTE_MAX: usize = 2_000;
const NOTES_KEPT: usize = 50;
const LOG_KEPT: usize = 500;
const TEXT_MAX: usize = 4_000;
/// The document kinds a citation may name: documents outrank an agent's code knowledge.
const CITABLE: [&str; 5] = ["drawing", "schedule", "spec", "submittal", "rfi"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("project.briefing", "Project Briefing", run_briefing)
            .params("{} → {revision, built: {level: {discipline: count}}, review: {open, deferred, resolved, byKind, top: [...]}, recent: [...], notes: [...], next: {call, why}, text}: one page to resume from")
            .noundo(),
        CommandSpec::new("project.note", "Project Note", run_note).params("{text, author?: agent|user (agent)} → {revision, notes}: a handoff note kept in the drawing"),
        CommandSpec::new("review.list", "Review Queue", run_list)
            .params("{kind?: rfi|mep_guess|clash|concern|validation|question, status?: open|deferred|resolved|all (open), level?, cursor?, limit?: 1-50 (10)} → {total, byKind, items: [{id, kind, level, title}], next_cursor?}")
            .noundo(),
        CommandSpec::new("review.next", "Next Review Item", run_next)
            .params("{kind?, level?, id?: an item id (that item, open or not)} → {item: {id, kind, level, title, detail, candidates, documents, how}, remaining}: the most important open item and how to answer it")
            .noundo(),
        CommandSpec::new("review.resolve", "Resolve Review Item", run_resolve).params(
            "{id, answer, citation?: {kind: drawing|schedule|spec|submittal|rfi, document, ref?}, author?: agent|user (agent), concern?: text} → {resolved, changed}: items the documents speak to need a citation; a concern from code knowledge alone becomes a review item",
        ),
        CommandSpec::new("review.defer", "Defer Review Item", run_defer).params("{id, reason} → {deferred, changed}: left for a field decision"),
        CommandSpec::new("review.rfi_draft", "Draft RFI", run_rfi_draft).params("{id, question} → {rfi, changed}: the item onto the RFI list (rfi.list), with its sheets"),
        CommandSpec::new("review.batch", "Batch Resolve", run_batch).params(
            "{kind, level?, system?, answer, citation, author?, confirm?: token} → without confirm {preview: {count, ids}, token}; with the preview's token {applied}: one answer for many identical items",
        ),
        CommandSpec::new("review.approve", "Approve Agent Work", run_approve).params("{ids?: [item ids], all?: bool} → {approved}: a person accepts what agents decided"),
        CommandSpec::new("review.log", "Agent Work", run_log)
            .menu(&["Tools", "Agent Work"])
            .params("{author?: agent|user, approved?: bool, cursor?, limit?} → {total, entries: [{rev, author, action, item, summary, citation, approved}], next_cursor?}")
            .noundo(),
        CommandSpec::new("bim.build", "Build BIM Model", run_build).params(
            "{level?: name, discipline?: architecture|structure|mep|all, force?: bool (build even when the import check failed)} → {level, built: {discipline: count}, unchanged, warnings, drafted?}: builds the model from the building spec (the same spec gives the same model); with no spec yet, drafts one from the open print set (or the plan drawing) first",
        ),
    ]
}

// ---- State ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct State {
    revision: u64,
    notes: Vec<Note>,
    decisions: BTreeMap<String, Decision>,
    log: Vec<LogEntry>,
    concerns: Vec<Concern>,
    /// The element counts of the last build, per level and discipline.
    last_build: BTreeMap<String, BTreeMap<String, usize>>,
    /// The building spec the last build was made from (a hash of its text).
    last_build_spec: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Note {
    rev: u64,
    author: String,
    text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Citation {
    kind: String,
    document: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    r#ref: String,
}

impl Citation {
    fn text(&self) -> String {
        if self.r#ref.is_empty() { format!("{} {}", self.kind, self.document) } else { format!("{} {}, {}", self.kind, self.document, self.r#ref) }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Decision {
    /// `resolved`, `deferred`, `rfi`.
    status: String,
    answer: Value,
    citation: Option<Citation>,
    author: String,
    rev: u64,
    approved: bool,
    title: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct LogEntry {
    rev: u64,
    author: String,
    action: String,
    item: String,
    summary: String,
    citation: Option<String>,
    approved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Concern {
    id: String,
    /// The item it is about (or empty).
    about: String,
    level: String,
    reasoning: String,
    author: String,
    rev: u64,
}

fn load(d: &Drawing) -> State {
    d.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
}

fn save(s: &mut Session, st: &mut State) -> Result<()> {
    st.notes = tail(std::mem::take(&mut st.notes), NOTES_KEPT);
    st.log = tail(std::mem::take(&mut st.log), LOG_KEPT);
    let doc = s.doc_mut()?;
    // Other state kept beside ours in the same JSON (the rules engine's `rulemodel`, the
    // guided job's) stays.
    let mut root: Value = doc.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let (Some(o), Ok(Value::Object(ours))) = (root.as_object_mut(), serde_json::to_value(&*st)) {
        o.extend(ours);
    }
    doc.agent = serde_json::to_string(&root).ok();
    Ok(())
}

fn tail<T>(mut v: Vec<T>, keep: usize) -> Vec<T> {
    let n = v.len();
    if n > keep {
        v.drain(..n - keep);
    }
    v
}

fn log(st: &mut State, author: &str, action: &str, item: &str, summary: &str, citation: Option<&Citation>) {
    st.log.push(LogEntry {
        rev: st.revision,
        author: author.into(),
        action: action.into(),
        item: item.into(),
        summary: cut(summary, 300),
        citation: citation.map(Citation::text),
        approved: author != "agent",
    });
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

/// FNV-1a: stable ids and tokens.
fn hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h & 0xffff_ffff)
}

fn author(s: &Session, p: &Value) -> Result<String> {
    let requested = str_param(p, "author").unwrap_or("agent");
    if s.agent.from_agent {
        if requested == "user" {
            return Err(bad("review", "an agent's work is tagged \"agent\" — only a person's call uses author: \"user\""));
        }
        return Ok("agent".to_string());
    }
    match requested {
        a @ ("agent" | "user") => Ok(a.to_string()),
        other => Err(bad("review", format!("author `{}` isn't known — use agent or user", cut(other, 20)))),
    }
}

// ---- The queue -----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Item {
    id: String,
    kind: &'static str,
    level: String,
    title: String,
    detail: Value,
    candidates: Vec<Value>,
    documents: Vec<String>,
    /// The documents speak to it: changing it needs a citation.
    documented: bool,
    rank: f64,
    /// Where it lives for applying an answer: the RFI id, the MEP run index, the clash.
    key: Value,
    /// For batches: the system of a run.
    system: String,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Every review item, ranked (most important first), with no decisions applied.
fn items(s: &mut Session) -> Vec<Item> {
    let mut out = Vec::new();
    // The prints' gaps and conflicts.
    if let Ok(v) = s.execute("rfi.list", &json!({})) {
        for r in v["items"].as_array().into_iter().flatten() {
            if r["status"].as_str().is_some_and(|x| x != "open") {
                continue;
            }
            let about = r["about"].as_str().unwrap_or_default();
            let level = r["level"].as_str().unwrap_or_default().to_string();
            let at = r["at"].as_array().map(|a| a.iter().filter_map(Value::as_f64).map(r1).collect::<Vec<_>>()).unwrap_or_default();
            let id = format!("RV-R{}", hash(&format!("{}|{about}|{level}|{at:?}|{}", r["kind"], r["issue"])));
            let rank = match r["severity"].as_str() {
                Some("high") => 0.0,
                Some("medium") => 1.0,
                _ => 2.0,
            };
            let mut cands = Vec::new();
            for k in ["proposed", "assumed"] {
                if let Some(t) = r[k].as_str().filter(|t| !t.is_empty()) {
                    cands.push(json!({ "answer": t, "from": k }));
                }
            }
            out.push(Item {
                id,
                kind: "rfi",
                level,
                title: cut(r["title"].as_str().unwrap_or(r["issue"].as_str().unwrap_or("gap in the prints")), 120),
                detail: json!({"issue": cut(r["issue"].as_str().unwrap_or_default(), 400), "about": cut(about, 120), "assumed": cut(r["assumed"].as_str().unwrap_or_default(), 200), "severity": r["severity"], "at": at}),
                candidates: cands,
                documents: r["sheets"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(8).collect(),
                documented: true,
                rank,
                key: r["id"].clone(),
                system: String::new(),
            });
        }
    }
    // Runs whose size or height is the system's default, not read from a tag.
    // `mep.list` answers a page at a time: walk every page (capped like the run list itself).
    let mut mep_items: Vec<Value> = Vec::new();
    let mut mep_systems: Option<Vec<Value>> = None;
    let mut cursor = 0u64;
    while let Ok(v) = s.execute("mep.list", &json!({"cursor": cursor, "limit": 200})) {
        if mep_systems.is_none() {
            mep_systems = Some(v["systems"].as_array().cloned().unwrap_or_default());
        }
        mep_items.extend(v["items"].as_array().into_iter().flatten().cloned());
        match v["next_cursor"].as_u64() {
            Some(n) if n > cursor && mep_items.len() < 100_000 => cursor = n,
            _ => break,
        }
    }
    if let Some(systems) = mep_systems {
        for r in &mep_items {
            let conf = r["confidence"].as_str().unwrap_or("low");
            if conf == "high" || r["reviewed"].as_bool() == Some(true) {
                continue;
            }
            let system = r["system"].as_str().unwrap_or_default().to_string();
            let level = r["level"].as_str().unwrap_or_default().to_string();
            // Identity: the plan objects the run was read from (stable while the plan is).
            let objects: Vec<&str> = r["objects"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let id = format!(
                "RV-M{}",
                hash(&format!("{system}|{level}|{objects:?}|{}", if objects.is_empty() { r["id"].to_string() } else { String::new() }))
            );
            let guessed: Vec<&str> = [("size", r["size_source"].as_str()), ("height", r["height_source"].as_str())]
                .into_iter()
                .filter(|(_, s)| *s != Some("tag"))
                .map(|(k, _)| k)
                .collect();
            let sy = systems.iter().find(|x| x["name"].as_str() == Some(system.as_str()));
            let mut cands = vec![
                json!({"answer": {"size_in": r["size_in"], "bottom_ft": r["bottoms_ft"].get(0)}, "from": "the system's default (what is built now)"}),
            ];
            // Sizes and heights tagged on other runs of the system.
            let mut seen = Vec::new();
            for o in mep_items.iter().filter(|o| o["system"] == r["system"] && o["confidence"] == "high") {
                let c = json!({"size_in": o["size_in"], "bottom_ft": o["bottoms_ft"].get(0)});
                if !seen.contains(&c) && seen.len() < 3 {
                    seen.push(c.clone());
                    cands.push(json!({"answer": c, "from": format!("a tagged {system} run on {}", o["level"].as_str().unwrap_or_default())}));
                }
            }
            out.push(Item {
                id,
                kind: "mep_guess",
                level: level.clone(),
                title: cut(&format!("{system} run on {level}: {} guessed", guessed.join(" and ")), 120),
                detail: json!({"system": system, "size_in": r["size_in"], "bottoms_ft": r["bottoms_ft"], "segments": r["segments"], "confidence": conf, "objects": objects.iter().take(4).collect::<Vec<_>>()}),
                candidates: cands,
                documents: vec![format!("the {level} plan sheet showing {system} (a size or height tag at the run)"), format!("the {} schedule or legend", sy.and_then(|x| x["kind"].as_str()).unwrap_or("system"))],
                documented: true,
                rank: if conf == "low" { 3.0 } else { 3.5 },
                key: r["id"].clone(),
                system,
            });
        }
    }
    // Clashes between trades: a coordination decision, not a document's (each with its objects
    // and a view of the spot, CLASH_ZOOM_HEIGHT drawing units high).
    if let Ok(v) = s.execute("model.clashes", &json!({})) {
        let listed = v["clashes"].as_array().map_or(0, Vec::len);
        let total = v["total"].as_u64().map_or(listed, |t| usize::try_from(t).unwrap_or(usize::MAX));
        if total > MAX_QUEUED_CLASHES {
            out.push(more_item("clash", total - MAX_QUEUED_CLASHES, total, "model.clashes {max_clashes: 50000} lists them all"));
        }
        for c in v["clashes"].as_array().into_iter().flatten().take(MAX_QUEUED_CLASHES) {
            let at = c["at"].as_array().map(|a| a.iter().filter_map(Value::as_f64).map(r1).collect::<Vec<_>>()).unwrap_or_default();
            let systems: Vec<String> = c["systems"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let id = format!("RV-C{}", hash(&format!("{}|{systems:?}|{at:?}|{}|{}", c["kind"], c["message"], c["handles"])));
            out.push(Item {
                id,
                kind: "clash",
                level: c["level"].as_str().unwrap_or_default().to_string(),
                title: cut(c["message"].as_str().unwrap_or("a clash"), 120),
                // What to look at: its objects, and the spot to zoom to (a run not built as
                // objects has none to select).
                detail: json!({
                    "kind": c["kind"], "systems": systems, "at": at, "by_in": c["by_in"], "handles": c["handles"],
                    "zoom": {"call": "zoom", "params": {"mode": "center", "center": at, "height": CLASH_ZOOM_HEIGHT}},
                }),
                candidates: vec![
                    json!({"answer": "reroute", "from": "move one run"}),
                    json!({"answer": "coordinated", "from": "accepted as drawn (say why)"}),
                ],
                documents: vec!["the plans of both trades at this spot".into()],
                documented: false,
                rank: 2.5,
                key: c.clone(),
                system: systems.first().cloned().unwrap_or_default(),
            });
        }
    }
    out.sort_by(|a, b| a.rank.total_cmp(&b.rank).then(a.id.cmp(&b.id)));
    out
}

fn concern_items(st: &State) -> Vec<Item> {
    st.concerns
        .iter()
        .map(|c| Item {
            id: c.id.clone(),
            kind: "concern",
            level: c.level.clone(),
            title: cut(&format!("concern ({}): {}", c.author, c.reasoning), 120),
            detail: json!({"about": c.about, "reasoning": c.reasoning}),
            candidates: vec![
                json!({"answer": "the documents already cover it (cite where)", "from": "check first"}),
                json!({"answer": "send as an RFI", "from": "review.rfi_draft"}),
            ],
            documents: vec!["the documents for the item it is about (does a schedule or detail already satisfy it?)".into()],
            documented: true,
            rank: 1.5,
            key: Value::Null,
            system: String::new(),
        })
        .collect()
}

fn status_of(st: &State, id: &str) -> &'static str {
    match st.decisions.get(id).map(|d| d.status.as_str()) {
        Some("deferred") => "deferred",
        Some("orphaned") => "orphaned",
        Some(_) => "resolved",
        None => "open",
    }
}

/// Clashes and validation failures queued at most; past that, one item says how many more there
/// are and where they are listed.
const MAX_QUEUED_CLASHES: usize = 2_000;
const MAX_QUEUED_FAILURES: usize = 2_000;

/// The height of the view a clash's review item zooms to: 20' in an inch drawing.
const CLASH_ZOOM_HEIGHT: f64 = 240.0;

/// The item saying `more` of `total` findings of a kind aren't queued, and the call that lists
/// them (first in the queue: the agent knows before it works through the rest).
fn more_item(kind: &'static str, more: usize, total: usize, call: &str) -> Item {
    Item {
        id: format!("RV-M{}", hash(&format!("more|{kind}|{total}"))),
        kind,
        level: String::new(),
        title: format!("{more} more of {total} not queued: {call}"),
        detail: json!({"not_queued": more, "total": total, "list_with": call}),
        candidates: Vec::new(),
        documents: Vec::new(),
        documented: false,
        rank: 0.0,
        key: json!({"not_queued": kind}),
        system: String::new(),
    }
}

/// Validation failures as review items (MUST 3: Q319/Q320 wiring).
fn validation_items(s: &mut Session) -> Vec<Item> {
    // As many as the queue takes (bim.validate lists 500 by default), and the count of all.
    let report = match s.execute("bim.validate", &json!({"limit": MAX_QUEUED_FAILURES})) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let listed = report["failures"].as_array().map_or(0, Vec::len).min(MAX_QUEUED_FAILURES);
    let failures = report["total"].as_u64().map_or(listed, |n| usize::try_from(n).unwrap_or(usize::MAX)).max(listed);
    if failures > listed {
        out.push(more_item("validation", failures - listed, failures, "bim.validate {\"limit\": 5000} lists them all"));
    }
    for f in report["failures"].as_array().into_iter().flatten().take(MAX_QUEUED_FAILURES) {
        let fid = f["id"].as_str().unwrap_or_default();
        let rule = f["rule"].as_str().unwrap_or_default();
        let level = f["level"].as_str().unwrap_or_default().to_string();
        let severity = f["severity"].as_str().unwrap_or("medium");
        let fix = f["fix"].as_str().unwrap_or_default();
        let element = f["element"].as_str().unwrap_or_default();
        let id = format!("RV-V{}", hash(&format!("validation|{fid}")));
        let rank = match severity {
            "high" => 0.5,
            "medium" => 1.5,
            _ => 2.5,
        };
        out.push(Item {
            id,
            kind: "validation",
            level,
            title: cut(&format!("{rule}: {}", f["expected"].as_str().unwrap_or_default()), 120),
            detail: json!({
                "failure_id": fid, "rule": rule, "element": element,
                "expected": f["expected"], "found": f["found"],
                "severity": severity, "fix": fix,
            }),
            candidates: vec![
                json!({"answer": fix, "from": "the validator's suggestion"}),
                json!({"answer": "override: the model is correct as-is", "from": "mark as override"}),
            ],
            documents: {
                let sheet = f.get("source").and_then(|s| s["sheet"].as_str()).unwrap_or_default();
                if sheet.is_empty() { vec![] } else { vec![sheet.to_string()] }
            },
            documented: true,
            rank,
            key: json!({"failure_id": fid, "element": element}),
            system: String::new(),
        });
    }
    out
}

/// The Validate run's judged findings (Q6116): a person accepts (a real error) or rejects (a
/// false alarm) each; the answer is kept and `validate.status` reports the false-alarm rate.
fn judged_items(s: &Session) -> Vec<Item> {
    let Ok(d) = s.doc() else { return Vec::new() };
    super::validate_run::judge_items(d)
        .into_iter()
        .map(|j| Item {
            id: j.id,
            kind: "validation",
            level: j.level,
            title: j.title,
            detail: j.detail,
            candidates: vec![
                json!({"answer": "accept", "from": "it is a real error: the model differs from the prints"}),
                json!({"answer": "reject", "from": "a false alarm: the model matches the prints"}),
            ],
            documents: j.documents,
            documented: false,
            rank: j.rank,
            key: j.key,
            system: String::new(),
        })
        .collect()
}

/// Unresolved device marks queued at most (one question per mark and sheet), and the symbols
/// each lists.
const MAX_QUEUED_QUESTIONS: usize = 500;
const QUESTION_SYMBOLS: usize = 8;

/// The device marks nothing resolved (no legend, schedule, panel schedule or known list says what
/// they are; the BOM lists them as UNRESOLVED): a question per mark and sheet, naming the sheet,
/// its symbols and the text printed round them. The answer is the kind (`luminaire`,
/// `equipment connection`) or `not a device`.
fn question_items(s: &Session) -> Vec<Item> {
    let Some(sp) = s.doc().ok().and_then(|d| d.building_spec.as_deref()).and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok()) else {
        return Vec::new();
    };
    let mut groups: BTreeMap<(String, String), Vec<&cadaddy_walk3d::tags::ElementTag>> = BTreeMap::new();
    for t in sp.tags.iter().filter(|t| t.id.starts_with("device:") && t.fields.contains_key("unresolved")) {
        let field = |k: &str| t.fields.get(k).map_or_else(String::new, |f| f.value.clone());
        groups.entry((field("type"), field("sheet"))).or_default().push(t);
    }
    groups
        .into_iter()
        .take(MAX_QUEUED_QUESTIONS)
        .map(|((mark, sheet), tags)| {
            let field = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| t.fields.get(k).map_or_else(String::new, |f| f.value.clone());
            let levels: std::collections::BTreeSet<&str> = tags.iter().map(|t| t.level.as_str()).collect();
            let level = if levels.len() == 1 { levels.into_iter().next().unwrap_or_default().to_string() } else { String::new() };
            let symbols: Vec<String> = tags.iter().map(|t| field(t, "symbol")).filter(|h| !h.is_empty()).take(QUESTION_SYMBOLS).collect();
            let near: Vec<String> =
                tags.iter().map(|t| field(t, "unresolved")).collect::<std::collections::BTreeSet<_>>().into_iter().take(3).collect();
            Item {
                id: format!("RV-Q{}", hash(&format!("question|{mark}|{sheet}"))),
                kind: "question",
                level,
                title: cut(&format!("What is device mark {mark} on {sheet}? {} placed; no legend, schedule or panel schedule says", tags.len()), 120),
                detail: json!({
                    "mark": mark, "sheet": sheet, "count": tags.len(), "symbols": symbols, "near_text": near,
                    "elements": tags.iter().take(QUESTION_SYMBOLS).map(|t| t.id.as_str()).collect::<Vec<_>>(),
                }),
                candidates: vec![
                    json!({"answer": "luminaire", "from": "a fixture type the lighting schedule lists"}),
                    json!({"answer": "equipment connection", "from": "a tag of the equipment it powers"}),
                    json!({"answer": "not a device", "from": "a grid bubble, a plant, a room or keynote tag: it comes out of the model"}),
                ],
                documents: vec![sheet.clone(), format!("the legend and schedules that speak for {sheet}"), "the panel schedules' load names".into()],
                documented: true,
                rank: 1.8,
                key: json!({"mark": mark, "sheet": sheet}),
                system: String::new(),
            }
        })
        .collect()
}

/// Fields whose documents disagree (`bim.form.record` met a different document's value): a
/// conflict per element and field, its values with their sources; the answer is the value that
/// holds, citing the document that settles it.
fn conflict_items(s: &Session) -> Vec<Item> {
    let Ok(d) = s.doc() else { return Vec::new() };
    super::forms::open_conflicts(d)
        .into_iter()
        .take(MAX_QUEUED_FAILURES)
        .map(|c| {
            let level = d.model.get(cadaddy_doc::Handle(u64::from_str_radix(&c.handle, 16).unwrap_or(0))).and_then(|e| super::elemprops::level_of(e)).unwrap_or_default();
            let said: Vec<String> = c
                .values
                .iter()
                .take(4)
                .map(|g| format!("{} ({}{})", g.value.as_str().map_or_else(|| g.value.to_string(), str::to_string), g.source.kind.as_str(), g.source.doc.as_deref().map(|x| format!(" {x}")).unwrap_or_default()))
                .collect();
            Item {
                id: format!("RV-F{}", hash(&format!("conflict|{}", c.id))),
                kind: "conflict",
                level,
                title: cut(&format!("{} {} {}: the documents disagree: {}", c.kind, c.handle, c.key, said.join(" vs ")), 120),
                detail: json!({"conflict": c.id, "handle": c.handle, "kind": c.kind, "key": c.key, "values": c.values}),
                candidates: c.values.iter().take(4).map(|g| json!({"answer": g.value, "from": format!("{} {}", g.source.kind.as_str(), g.source.doc.clone().unwrap_or_default()).trim().to_string()})).collect(),
                documents: c.values.iter().filter_map(|g| g.source.doc.clone()).take(8).collect(),
                documented: true,
                rank: 1.2,
                key: json!({"conflict": c.id}),
                system: String::new(),
            }
        })
        .collect()
}

/// An answered device-mark question into the model: its devices take the kind (`not a device`
/// takes them out), the answer and its citation their source.
fn answer_question(s: &mut Session, item: &Item, answer: &Value, cite: Option<&Citation>, who: &str) -> Result<()> {
    let Some(kind) = answer.as_str().map(str::trim).filter(|a| !a.is_empty() && a.len() <= 60) else {
        return Err(bad(
            "review.resolve",
            "a device mark's answer is its kind (\"luminaire\", \"equipment connection\", \"receptacle\") or \"not a device\"",
        ));
    };
    let (mark, sheet) = (item.key["mark"].as_str().unwrap_or_default(), item.key["sheet"].as_str().unwrap_or_default());
    let (mut sp, _) = current(s, "review.resolve")?;
    let ours = |t: &cadaddy_walk3d::tags::ElementTag| {
        let field = |k: &str| t.fields.get(k).map_or("", |f| f.value.as_str());
        t.id.starts_with("device:") && t.fields.contains_key("unresolved") && field("type") == mark && field("sheet") == sheet
    };
    let source = match cite {
        Some(c) => format!("answered ({who}): {}", c.text()),
        None => format!("answered ({who})"),
    };
    if kind.eq_ignore_ascii_case("not a device") {
        sp.tags.retain(|t| !ours(t));
    } else {
        for t in sp.tags.iter_mut().filter(|t| ours(t)) {
            t.kind = kind.to_ascii_lowercase().replace(' ', "_");
            t.set("device_type", format!("{} {mark}", kind.to_ascii_lowercase()), "user");
            t.set("kind_source", source.clone(), "user");
            t.fields.remove("unresolved");
        }
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// Every decision kept in the drawing: item id → (status, answer).
pub(crate) fn decisions(d: &Drawing) -> BTreeMap<String, (String, Value)> {
    load(d).decisions.into_iter().map(|(id, x)| (id, (x.status, x.answer))).collect()
}

fn all_items(s: &mut Session, st: &State) -> Vec<Item> {
    let mut v = items(s);
    v.extend(concern_items(st));
    v.extend(validation_items(s));
    v.extend(judged_items(s));
    v.extend(question_items(s));
    v.extend(conflict_items(s));
    v.sort_by(|a, b| a.rank.total_cmp(&b.rank).then(a.id.cmp(&b.id)));
    v
}

fn compact(i: &Item, st: &State) -> Value {
    json!({ "id": i.id, "kind": i.kind, "level": i.level, "title": i.title, "status": status_of(st, &i.id) })
}

fn how(i: &Item) -> Value {
    if i.key.get("judge").is_some() {
        return json!({"call": "review.resolve", "example": {"id": i.id, "answer": "accept", "author": "user"}, "or": "answer \"reject\" when it is a false alarm (a person answers: author user)"});
    }
    match i.kind {
        "mep_guess" => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": {"size_in": [14, 10], "bottom_ft": 10.5}, "citation": {"kind": "drawing", "document": "M-101", "ref": "tag at grid C-4"}}})
        }
        "rfi" => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": "Type 3A, full height to deck", "citation": {"kind": "rfi", "document": "RFI-012"}}, "or": "review.rfi_draft {id, question} to ask the design team"})
        }
        "clash" => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": "coordinated: sprinkler main drops below the duct per FP-101"}, "or": "review.defer {id, reason}"})
        }
        "conflict" => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": "20A 125V", "citation": {"kind": "submittal", "document": "the approved product data", "ref": "page 2"}}, "or": "review.rfi_draft {id, question} when no document settles it"})
        }
        "question" => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": "luminaire", "citation": {"kind": "schedule", "document": "E-001", "ref": "light fixture schedule, type C"}}, "or": "answer \"not a device\" for a grid bubble or another trade's tag"})
        }
        _ => {
            json!({"call": "review.resolve", "example": {"id": i.id, "answer": "covered: GFCI breaker on circuit 12", "citation": {"kind": "schedule", "document": "E-601 panel L1"}}})
        }
    }
}

/// Keep an answer under the cap: drop list entries from the end and say so.
fn capped(mut v: Value, list: &str, max: usize) -> Value {
    let mut truncated = false;
    while v.to_string().len() > max {
        let Some(a) = v.get_mut(list).and_then(Value::as_array_mut) else { break };
        if a.pop().is_none() {
            break;
        }
        truncated = true;
    }
    if truncated && let Some(o) = v.as_object_mut() {
        o.insert("truncated".into(), json!(true));
    }
    v
}

fn page(p: &Value) -> (usize, usize) {
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE_DEFAULT, |n| (n as usize).clamp(1, PAGE_MAX));
    let cursor = match p.get("cursor") {
        Some(Value::String(c)) => c.trim_start_matches("c").parse::<usize>().unwrap_or(0),
        Some(v) => v.as_u64().map_or(0, |n| n as usize),
        None => 0,
    };
    (cursor.min(1_000_000), limit)
}

fn find_item(s: &mut Session, st: &State, cmd: &str, p: &Value) -> Result<Item> {
    let id = str_param(p, "id").ok_or_else(|| bad(cmd, "`id` is required: an item id from review.next or review.list (e.g. RV-M1a2b3c4d)"))?;
    all_items(s, st)
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| bad(cmd, format!("no review item `{}` — call review.list (the item may have been settled by a rebuild)", cut(id, 40))))
}

fn citation(p: &Value, cmd: &str) -> Result<Option<Citation>> {
    let Some(c) = p.get("citation").filter(|c| !c.is_null()) else { return Ok(None) };
    let c: Citation = serde_json::from_value(c.clone()).map_err(|_| bad(cmd, "`citation` is {kind, document, ref?}"))?;
    if !CITABLE.contains(&c.kind.as_str()) {
        return Err(bad(
            cmd,
            format!(
                "a citation's kind is one of {} — documents outrank code knowledge; for a concern without a document pass `concern`",
                CITABLE.join(", ")
            ),
        ));
    }
    if c.document.trim().is_empty() {
        return Err(bad(cmd, "a citation names its document (a sheet number, schedule, spec section, submittal or RFI number)"));
    }
    Ok(Some(Citation { kind: c.kind, document: cut(c.document.trim(), 120), r#ref: cut(c.r#ref.trim(), 200) }))
}

// ---- Commands ------------------------------------------------------------------------------------

/// How many review items of the active drawing are open (the project hub's list shows it).
pub(crate) fn open_review_count(s: &mut Session) -> Option<usize> {
    let st = load(s.doc().ok()?);
    let all = all_items(s, &st);
    Some(all.iter().filter(|i| status_of(&st, &i.id) == "open").count())
}

fn run_briefing(s: &mut Session, _p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let has_spec = s.doc()?.building_spec.is_some();
    let built = built_counts(s.doc()?);
    let all = all_items(s, &st);
    let open: Vec<&Item> = all.iter().filter(|i| status_of(&st, &i.id) == "open").collect();
    let deferred = all.iter().filter(|i| status_of(&st, &i.id) == "deferred").count();
    let orphaned = st.decisions.values().filter(|d| d.status == "orphaned").count();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for i in &open {
        *by_kind.entry(i.kind).or_default() += 1;
    }
    let top: Vec<Value> = open.iter().take(5).map(|i| json!({"id": i.id, "kind": i.kind, "level": i.level, "title": cut(&i.title, 90)})).collect();
    let recent: Vec<Value> = st
        .log
        .iter()
        .rev()
        .take(5)
        .map(|e| json!({"rev": e.rev, "author": e.author, "action": e.action, "item": e.item, "summary": cut(&e.summary, 120)}))
        .collect();
    let notes: Vec<Value> = st.notes.iter().rev().take(5).map(|n| json!({"rev": n.rev, "author": n.author, "text": cut(&n.text, 300)})).collect();
    let next = if !has_spec && (s.printset_project.is_some() || !s.doc()?.model.is_empty()) {
        json!({"call": "bim.build {}", "why": "there is no building spec yet: the build drafts it from the prints, then builds it"})
    } else if !has_spec {
        json!({"call": "printset.open {path}", "why": "no prints yet: open the folder of prints (bim_job import {path}), then build"})
    } else if built.is_empty() {
        json!({"call": "bim.build {}", "why": "the building spec isn't built yet"})
    } else if !st.last_build_spec.is_empty() && st.last_build_spec != spec_hash(s.doc()?) && open.is_empty() {
        // Answers changed the spec since the build (Q320: the weak agent took off quantities
        // from a model older than its own answers).
        json!({"call": "bim.build {}", "why": "the building spec changed since the last build (review answers): build again before the takeoff"})
    } else if let Some(i) = open.first() {
        json!({"call": format!("review.next {{}} (top: {})", i.id), "why": format!("{} open review item(s)", open.len())})
    } else {
        json!({"call": "bim.takeoff {} or export {path: \"model.ifc\"}", "why": "built, nothing open to review"})
    };
    let built_text: Vec<String> =
        built.iter().map(|(l, m)| format!("{l}: {}", m.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "))).collect();
    let mut text = format!("Revision {}. ", st.revision);
    text += &if built.is_empty() { "Nothing built yet. ".to_string() } else { format!("Built: {}. ", built_text.join("; ")) };
    text += &format!(
        "Review: {} open ({}), {} deferred, {} decided{}. ",
        open.len(),
        by_kind.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", "),
        deferred,
        st.decisions.values().filter(|d| d.status == "resolved").count(),
        if orphaned > 0 { format!(", {orphaned} orphaned") } else { String::new() }
    );
    if let Some(n) = st.notes.last() {
        text += &format!("Last note ({}): {} ", n.author, cut(&n.text, 300));
    }
    // The estimate, once there is one (Q327): its total, whether the validator is clean, and how
    // many findings are open.
    let has_estimate = has_spec && super::estimate::state(s, "bim_job").is_ok_and(|(_, st)| st.contains_key("baseline"));
    let estimate = if has_estimate {
        s.execute("estimate.status", &json!({})).ok().map(|e| {
            let open = e["findings"].as_array().map_or(0, Vec::len);
            json!({"total": e["totals"]["total"], "ok": e["ok"], "open_findings": open, "lines": e["summary"]["lines"], "call": "estimate.status {}"})
        })
    } else {
        None
    };
    if let Some(e) = &estimate {
        text += &format!("Estimate: ${} total, {} open finding(s). ", e["total"], e["open_findings"]);
    }
    // MUST 3: validation score in the briefing.
    let validation = s.execute("bim.validate", &json!({})).ok().map(|r| {
        let by_check = r.get("byCheck").cloned().unwrap_or(Value::Null);
        json!({
            "pass": r["pass"],
            "score": r["score"],
            "total": r["total"],
            "byCheck": by_check,
        })
    });
    if let Some(v) = &validation {
        let pass = v["pass"].as_bool().unwrap_or(false);
        let score = v["score"].as_f64().unwrap_or(0.0);
        let total = v["total"].as_u64().unwrap_or(0);
        text +=
            &format!("Validation: {} (score {score}, {total} failure{}). ", if pass { "PASS" } else { "FAIL" }, if total == 1 { "" } else { "s" });
    }
    text += &format!("Next: {} — {}.", next["call"].as_str().unwrap_or_default(), next["why"].as_str().unwrap_or_default());
    let mut v = json!({
        "revision": st.revision,
        "built": built,
        "review": {"open": open.len(), "deferred": deferred, "resolved": st.decisions.values().filter(|d| d.status == "resolved").count(), "orphaned": orphaned, "byKind": by_kind, "top": top},
        "recent": recent,
        "notes": notes,
        "next": next,
        "text": cut(&text, 2_000),
    });
    if let Some(e) = estimate {
        v["estimate"] = e;
    }
    if let Some(val) = validation {
        v["validation"] = val;
    }
    // Within the page: trim the lists, longest first.
    let mut v = capped(v, "recent", BRIEFING_MAX);
    v = capped(v, "notes", BRIEFING_MAX);
    if v.to_string().len() > BRIEFING_MAX
        && let Some(r) = v.get_mut("review").and_then(Value::as_object_mut)
    {
        r.insert("top".into(), json!([]));
    }
    if v.to_string().len() > BRIEFING_MAX
        && let Some(o) = v.as_object_mut()
    {
        // A huge building: per level totals only.
        let totals: BTreeMap<String, usize> = built.iter().map(|(l, m)| (l.clone(), m.values().sum())).take(200).collect();
        o.insert("built".into(), json!(totals));
    }
    Ok(v)
}

/// Built elements per level and discipline (from the model's build tags).
fn built_counts(d: &Drawing) -> BTreeMap<String, BTreeMap<String, usize>> {
    let mut m: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for e in d.model.iter() {
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)) else { continue };
        let tags: Vec<&str> = x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();
        let (Some(part), Some(level)) = (tags.first(), tags.get(1)) else { continue };
        let disc = discipline(part);
        *m.entry((*level).to_string()).or_default().entry(disc.into()).or_default() += 1;
    }
    m
}

fn spec_hash(d: &Drawing) -> String {
    d.building_spec.as_deref().map_or_else(String::new, hash)
}

fn discipline(part: &str) -> &'static str {
    match part {
        "mep" => "mep",
        "column" | "beam" | "grid" | "gridcolumn" | "slab" | "joist" => "structure",
        _ => "architecture",
    }
}

fn run_note(s: &mut Session, p: &Value) -> Result<Value> {
    let text = str_param(p, "text")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("project.note", "`text` is required: what the next agent or person should know"))?;
    let who = author(s, p)?;
    let mut st = load(s.doc()?);
    let text = cut(text, NOTE_MAX);
    if st.notes.last().is_some_and(|n| n.text == text && n.author == who) {
        return Ok(json!({"revision": st.revision, "notes": st.notes.len(), "changed": false}));
    }
    st.revision += 1;
    st.notes.push(Note { rev: st.revision, author: who.clone(), text: text.clone() });
    log(&mut st, &who, "note", "", &text, None);
    save(s, &mut st)?;
    Ok(json!({"revision": st.revision, "notes": st.notes.len(), "changed": true}))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let kind = str_param(p, "kind");
    if kind.is_some_and(|k| !["rfi", "mep_guess", "clash", "concern", "validation", "question", "conflict"].contains(&k)) {
        return Err(bad("review.list", "`kind` is rfi, mep_guess, clash, concern, validation, question or conflict"));
    }
    let status = str_param(p, "status").unwrap_or("open");
    if !["open", "deferred", "resolved", "orphaned", "all"].contains(&status) {
        return Err(bad("review.list", "`status` is open, deferred, resolved, orphaned or all"));
    }
    let level = str_param(p, "level");
    let (cursor, limit) = page(p);
    let mut all: Vec<Value> = all_items(s, &st)
        .iter()
        .filter(|i| kind.is_none_or(|k| i.kind == k))
        .filter(|i| level.is_none_or(|l| i.level.eq_ignore_ascii_case(l)))
        .filter(|i| status == "all" || status_of(&st, &i.id) == status)
        .map(|i| compact(i, &st))
        .collect();
    // Orphaned decisions: their runs are gone from the plans but the decisions are kept.
    if status == "orphaned" || status == "all" {
        for (id, d) in &st.decisions {
            if d.status != "orphaned" {
                continue;
            }
            all.push(json!({"id": id, "kind": "orphaned", "level": "", "title": d.title, "status": "orphaned"}));
        }
    }
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for i in &all {
        let k = i["kind"].as_str().unwrap_or("unknown");
        *by_kind.entry(k).or_default() += 1;
    }
    let items: Vec<&Value> = all.iter().skip(cursor).take(limit).collect();
    let next = (cursor + items.len() < all.len()).then(|| format!("c{}", cursor + items.len()));
    let mut v = json!({"total": all.len(), "byKind": by_kind, "items": items});
    if let (Some(n), Some(o)) = (next, v.as_object_mut()) {
        o.insert("next_cursor".into(), json!(n));
    }
    Ok(capped(v, "items", ANSWER_MAX))
}

fn run_next(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let kind = str_param(p, "kind");
    let level = str_param(p, "level");
    let want = str_param(p, "id");
    let open: Vec<Item> = all_items(s, &st)
        .into_iter()
        .filter(|i| want.is_some_and(|w| w == i.id) || (want.is_none() && status_of(&st, &i.id) == "open"))
        .filter(|i| kind.is_none_or(|k| i.kind == k))
        .filter(|i| level.is_none_or(|l| i.level.eq_ignore_ascii_case(l)))
        .collect();
    let Some(i) = open.first() else {
        let built = !built_counts(s.doc()?).is_empty();
        return Ok(
            json!({"item": null, "remaining": 0, "message": if built { "nothing open — call project.briefing, or takeoff / export" } else { "nothing to review yet — call bim.build {} first" }}),
        );
    };
    let v = json!({
        "item": {"id": i.id, "kind": i.kind, "level": i.level, "title": i.title, "detail": i.detail, "candidates": i.candidates, "documents": i.documents, "needs_citation": i.documented, "how": how(i)},
        "remaining": open.len() - 1,
    });
    Ok(capped(v, "candidates", ANSWER_MAX))
}

fn run_resolve(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load(s.doc()?);
    let item = find_item(s, &st, "review.resolve", p)?;
    let who = author(s, p)?;
    let answer = p.get("answer").filter(|a| !a.is_null()).cloned();
    let cite = citation(p, "review.resolve")?;
    // Code knowledge alone doesn't change a documented item: it becomes a concern to review.
    if item.documented && cite.is_none() {
        let Some(reason) = str_param(p, "concern").map(str::trim).filter(|t| !t.is_empty()) else {
            return Err(bad(
                "review.resolve",
                format!(
                    "{} is something the documents speak to: cite one (citation: {{kind: drawing|schedule|spec|submittal|rfi, document, ref}}). A concern from code knowledge alone: pass concern: \"…\" and it is raised for review (check first how the documents already satisfy it)",
                    item.id
                ),
            ));
        };
        let reason = cut(reason, TEXT_MAX);
        let id = format!("RV-K{}", hash(&format!("{}|{reason}", item.id)));
        if st.concerns.iter().any(|c| c.id == id) {
            return Ok(json!({"raised": id, "changed": false}));
        }
        st.revision += 1;
        st.concerns.push(Concern {
            id: id.clone(),
            about: item.id.clone(),
            level: item.level.clone(),
            reasoning: reason.clone(),
            author: who.clone(),
            rev: st.revision,
        });
        log(&mut st, &who, "concern", &item.id, &reason, None);
        save(s, &mut st)?;
        return Ok(json!({"raised": id, "changed": false, "message": "raised as a review item: the model is unchanged until a document settles it"}));
    }
    let Some(answer) = answer else {
        return Err(bad("review.resolve", format!("`answer` is required — e.g. {}", how(&item)["example"])));
    };
    let answer = clamp_value(answer);
    if let Some(d) = st.decisions.get(&item.id)
        && d.status == "resolved"
        && d.answer == answer
        && d.citation == cite
    {
        return Ok(json!({"resolved": item.id, "changed": false}));
    }
    apply(s, &item, &answer, cite.as_ref(), &who)?;
    // The application may have changed the drawing's agent state? No: it touches the spec only.
    st.revision += 1;
    st.decisions.insert(
        item.id.clone(),
        Decision {
            status: "resolved".into(),
            answer: answer.clone(),
            citation: cite.clone(),
            author: who.clone(),
            rev: st.revision,
            approved: who != "agent",
            title: item.title.clone(),
        },
    );
    log(&mut st, &who, "resolve", &item.id, &format!("{} → {}", item.title, cut(&answer.to_string(), 160)), cite.as_ref());
    save(s, &mut st)?;
    Ok(json!({"resolved": item.id, "changed": true, "revision": st.revision}))
}

/// An answer's text and lists capped.
fn clamp_value(v: Value) -> Value {
    match v {
        Value::String(t) => Value::String(cut(&t, TEXT_MAX)),
        Value::Array(a) => Value::Array(a.into_iter().take(16).map(clamp_value).collect()),
        Value::Object(o) => Value::Object(o.into_iter().take(16).map(|(k, v)| (cut(&k, 40), clamp_value(v))).collect()),
        other => other,
    }
}

/// Put an answer into the model.
fn apply(s: &mut Session, item: &Item, answer: &Value, cite: Option<&Citation>, who: &str) -> Result<()> {
    match item.kind {
        "mep_guess" => {
            let size = answer.get("size_in").filter(|v| v.is_array());
            let bottom = answer.get("bottom_ft").and_then(Value::as_f64);
            let system = answer.get("system").and_then(Value::as_str);
            if size.is_none() && bottom.is_none() && system.is_none() {
                return Err(bad(
                    "review.resolve",
                    "a run's answer is {size_in?: [w, h], bottom_ft?: feet, system?: name} — e.g. {\"size_in\": [14, 10], \"bottom_ft\": 10.5}",
                ));
            }
            if bottom.is_some_and(|b| !(b.is_finite() && (-50.0..=500.0).contains(&b))) {
                return Err(bad("review.resolve", "bottom_ft is feet above the level's floor"));
            }
            let mut params = json!({"id": item.key, "reviewed": true});
            if let Some(o) = params.as_object_mut() {
                if let Some(v) = size {
                    o.insert("size_in".into(), v.clone());
                }
                if let Some(b) = bottom {
                    o.insert("bottom_ft".into(), json!(b));
                }
                if let Some(sy) = system {
                    o.insert("system".into(), json!(sy));
                }
            }
            s.execute("mep.edit", &params).map(|_| ())
        }
        "rfi" => {
            let text = answer.as_str().map_or_else(|| answer.to_string(), str::to_string);
            let full = match cite {
                Some(c) => format!("{text} [{}; {who}]", c.text()),
                None => format!("{text} [{who}]"),
            };
            s.execute("rfi.set", &json!({"id": item.key, "status": "answered", "answer": full})).map(|_| ())
        }
        // A judged finding: a person says whether it is a real error; the decision is the record.
        "validation" if item.key.get("judge").is_some() => {
            if who != "user" {
                return Err(bad(
                    "review.resolve",
                    "a judged finding is accepted (a real error) or rejected (a false alarm) by a person: author \"user\" (the judge doesn't grade itself)",
                ));
            }
            super::validate_run::judge_answer(answer).map(|_| ()).map_err(|e| bad("review.resolve", e))
        }
        "question" => answer_question(s, item, answer, cite, who),
        "conflict" => {
            let id = item.key["conflict"].as_str().unwrap_or_default();
            super::forms::resolve_conflict(s, id, answer, &cite.map(Citation::text).unwrap_or_default(), who)
        }
        // Clashes and concerns: the decision is the record.
        _ => Ok(()),
    }
}

fn run_defer(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load(s.doc()?);
    let item = find_item(s, &st, "review.defer", p)?;
    let reason = str_param(p, "reason")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("review.defer", "`reason` is required: what the field has to decide"))?;
    let who = author(s, p)?;
    if st.decisions.get(&item.id).is_some_and(|d| d.status == "deferred") {
        return Ok(json!({"deferred": item.id, "changed": false}));
    }
    st.revision += 1;
    st.decisions.insert(
        item.id.clone(),
        Decision {
            status: "deferred".into(),
            answer: json!(cut(reason, TEXT_MAX)),
            citation: None,
            author: who.clone(),
            rev: st.revision,
            approved: who != "agent",
            title: item.title.clone(),
        },
    );
    log(&mut st, &who, "defer", &item.id, reason, None);
    save(s, &mut st)?;
    Ok(json!({"deferred": item.id, "changed": true, "revision": st.revision}))
}

fn run_rfi_draft(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load(s.doc()?);
    let item = find_item(s, &st, "review.rfi_draft", p)?;
    let question = str_param(p, "question")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("review.rfi_draft", "`question` is required: what to ask the design team"))?;
    let question = cut(question, TEXT_MAX);
    let who = author(s, p)?;
    let (mut sp, stored) = current(s, "review.rfi_draft")?;
    if !stored {
        return Err(bad("review.rfi_draft", "there is no building spec to keep the RFI in — call bspec.draft {} first"));
    }
    let about = item.title.clone();
    if sp.rfis.iter().any(|r| r.about == about && r.issue == question) {
        // The same question is already on the list (another item with the same title asked it):
        // this item is settled by it too, so it leaves the queue (Q320: the weak agent asked the
        // same RFI 1,984 times because the second item never left).
        if st.decisions.get(&item.id).is_some_and(|d| d.status == "rfi") {
            return Ok(json!({"rfi": about, "changed": false}));
        }
        st.revision += 1;
        st.decisions.insert(
            item.id.clone(),
            Decision {
                status: "rfi".into(),
                answer: json!(question),
                citation: None,
                author: who.clone(),
                rev: st.revision,
                approved: who != "agent",
                title: item.title.clone(),
            },
        );
        log(&mut st, &who, "rfi", &item.id, &format!("same question as an RFI already on the list: {question}"), None);
        save(s, &mut st)?;
        return Ok(json!({"rfi": about, "changed": true, "same_as_existing": true}));
    }
    if sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
        return Err(bad("review.rfi_draft", "the RFI list is full — answer or close some first (rfi.set)"));
    }
    let at = item
        .detail
        .get("at")
        .or_else(|| item.detail.get("start"))
        .and_then(Value::as_array)
        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)])
        .unwrap_or([0.0, 0.0]);
    let mut r = Rfi {
        level: item.level.clone(),
        at,
        about: about.clone(),
        issue: question.clone(),
        assumed: format!("drafted by {who} from review item {}", item.id),
        sheets: item.documents.iter().filter(|d| d.len() <= 12).cloned().collect(),
        ..Rfi::default()
    };
    r.id = format!("RFI-A{}", hash(&format!("{about}|{question}")));
    // Sent to the design team: it waits for their answer, not in the agent's open queue (Q320:
    // the weak agent got its own RFIs back as new items and handled them twice).
    r.status = cadaddy_walk3d::RfiStatus::Sent;
    sp.rfis.push(r);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    st.revision += 1;
    st.decisions.insert(
        item.id.clone(),
        Decision {
            status: "rfi".into(),
            answer: json!(question),
            citation: None,
            author: who.clone(),
            rev: st.revision,
            approved: who != "agent",
            title: item.title.clone(),
        },
    );
    log(&mut st, &who, "rfi", &item.id, &question, None);
    save(s, &mut st)?;
    Ok(json!({"rfi": about, "changed": true, "revision": st.revision}))
}

fn run_batch(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let kind = str_param(p, "kind").ok_or_else(|| bad("review.batch", "`kind` is required (e.g. mep_guess)"))?;
    let level = str_param(p, "level");
    let system = str_param(p, "system");
    let answer = clamp_value(
        p.get("answer")
            .filter(|a| !a.is_null())
            .cloned()
            .ok_or_else(|| bad("review.batch", "`answer` is required: the one answer for every matching item"))?,
    );
    let cite = citation(p, "review.batch")?;
    let matched: Vec<Item> = all_items(s, &st)
        .into_iter()
        .filter(|i| i.kind == kind && status_of(&st, &i.id) == "open")
        .filter(|i| level.is_none_or(|l| i.level.eq_ignore_ascii_case(l)))
        .filter(|i| system.is_none_or(|sy| i.system.eq_ignore_ascii_case(sy)))
        .collect();
    // Gaps, runs and concerns are what the documents speak to (clashes aren't).
    if (kind != "clash" || matched.iter().any(|i| i.documented)) && cite.is_none() {
        return Err(bad("review.batch", "these items are documented: a batch answer needs a citation (citation: {kind, document, ref})"));
    }
    let ids: Vec<&str> = matched.iter().map(|i| i.id.as_str()).collect();
    let token = format!("B{}", hash(&format!("{ids:?}|{answer}|{cite:?}")));
    match str_param(p, "confirm") {
        None => Ok(
            json!({"preview": {"count": ids.len(), "ids": ids.iter().take(20).collect::<Vec<_>>(), "answer": answer}, "token": token, "message": format!("call review.batch again with the same parameters and confirm: \"{token}\" to apply")}),
        ),
        Some(t) if t != token => Err(bad(
            "review.batch",
            "the confirm token doesn't match the preview (the queue or the parameters changed) — call review.batch without confirm to see it again",
        )),
        Some(_) => {
            let mut applied = 0;
            for i in &matched {
                let mut q = json!({"id": i.id, "answer": answer, "author": str_param(p, "author").unwrap_or("agent")});
                if let (Some(c), Some(o)) = (&cite, q.as_object_mut()) {
                    o.insert("citation".into(), serde_json::to_value(c).unwrap_or(Value::Null));
                }
                if s.execute("review.resolve", &q).is_ok_and(|v| v["changed"] == json!(true)) {
                    applied += 1;
                }
            }
            Ok(json!({"applied": applied, "of": matched.len()}))
        }
    }
}

fn run_approve(s: &mut Session, p: &Value) -> Result<Value> {
    if s.agent.from_agent {
        return Err(bad("review.approve", "only a person can approve agent decisions — ask the user to open Tools \u{25b8} Agent Work"));
    }
    let mut st = load(s.doc()?);
    let all = bool_or(p, "all", false);
    let ids: Vec<String> = p
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(10_000).map(str::to_string).collect())
        .unwrap_or_default();
    if !all && ids.is_empty() {
        return Err(bad("review.approve", "pass ids: [item ids] or all: true (review.log lists what agents decided)"));
    }
    let mut n = 0;
    for (id, d) in st.decisions.iter_mut() {
        if !d.approved && (all || ids.contains(id)) {
            d.approved = true;
            n += 1;
        }
    }
    for e in st.log.iter_mut() {
        if !e.approved && (all || ids.contains(&e.item)) {
            e.approved = true;
        }
    }
    if n == 0 {
        return Ok(json!({"approved": 0, "changed": false}));
    }
    st.revision += 1;
    log(&mut st, "user", "approve", "", &format!("approved {n} decision(s)"), None);
    save(s, &mut st)?;
    Ok(json!({"approved": n, "changed": true}))
}

fn run_log(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let who = str_param(p, "author");
    let approved = p.get("approved").and_then(Value::as_bool);
    let (cursor, limit) = page(p);
    let all: Vec<&LogEntry> =
        st.log.iter().rev().filter(|e| who.is_none_or(|w| e.author == w)).filter(|e| approved.is_none_or(|a| e.approved == a)).collect();
    let entries: Vec<Value> = all.iter().skip(cursor).take(limit).map(|e| serde_json::to_value(e).unwrap_or(Value::Null)).collect();
    let next = (cursor + entries.len() < all.len()).then(|| format!("c{}", cursor + entries.len()));
    let mut v = json!({"total": all.len(), "entries": entries});
    if let (Some(n), Some(o)) = (next, v.as_object_mut()) {
        o.insert("next_cursor".into(), json!(n));
    }
    Ok(capped(v, "entries", ANSWER_MAX))
}

/// `bim.build`: one undo step, however many commands it runs.
fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| build(s, p))
}

fn build(s: &mut Session, p: &Value) -> Result<Value> {
    // Gate: refuse to build if the import check failed, unless --force or discipline is mep
    // (the MEP geometry reader handles paper-scale and pen-layer drawings natively).
    let force = bool_or(p, "force", false);
    let disc = str_param(p, "discipline").unwrap_or("all");
    if !force
        && disc != "mep"
        && let Ok(d) = s.doc()
        && !super::import_check::passes(d)
    {
        let fails = super::import_check::failed_checks(d);
        let names: Vec<&str> = fails.iter().map(|(id, _)| id.as_str()).collect();
        return Err(bad("bim.build", format!("import check failed ({}); fix the import or pass `force: true` to override", names.join(", "))));
    }
    // No spec yet: draft it from the open print set, else from the plan drawing (Q5276: the
    // build an import suggests must work as given).
    let mut drafted = None;
    if !current(s, "bim.build")?.1 {
        let draft = if s.printset_project.is_some() {
            super::guided::draft_from_prints(s)?
        } else if s.doc().is_ok_and(|d| !d.model.is_empty()) {
            s.execute("bspec.draft", &json!({"store": true}))?
        } else {
            return Err(bad(
                "bim.build",
                "there is no building spec and nothing to draft one from — open the prints first (printset.open {path}, or bim_job import {path}) or a plan drawing",
            ));
        };
        drafted = Some(json!({"guesses": draft["guesses"].as_array().map_or(0, Vec::len), "call": "bspec.get {} shows the drafted spec"}));
    }
    let (sp, stored) = current(s, "bim.build")?;
    if !stored {
        return Err(bad("bim.build", "the building spec couldn't be drafted — call bspec.draft {store: true} to see why"));
    }
    let level = str_param(p, "level");
    if let Some(l) = level
        && sp.level(l).is_none()
    {
        let names: Vec<&str> = sp.levels.iter().map(|x| x.name.as_str()).take(50).collect();
        return Err(bad(
            "bim.build",
            format!("level `{}` isn't in the building spec — its levels are {}; call bspec.get to see them", cut(l, 40), names.join(", ")),
        ));
    }
    let disc = str_param(p, "discipline").unwrap_or("all");
    if !["architecture", "structure", "mep", "all"].contains(&disc) {
        return Err(bad("bim.build", "`discipline` is architecture, structure, mep or all"));
    }

    // Auto-read MEP data: when building with discipline mep or all, populate the spec's
    // MEP runs/equipment/terminals from the current drawing's geometry before building.
    // Only re-read when (a) the drawing has entities to read from and (b) the spec does
    // NOT already have MEP runs — pre-set runs (from bspec.set, a previous import or an
    // agent pipeline) are authoritative and must not be overwritten by a re-read.
    // (c) The sheet is not a notes/legend/schedule page — those have schedule-table lines
    // that the readers misidentify as MEP runs.
    let mut mep_warnings: Vec<String> = Vec::new();
    let has_drawing_entities = s.doc().is_ok_and(|d| !d.model.is_empty());
    let spec_has_mep = current(s, "bim.build").map(|(sp, _)| !sp.mep.runs.is_empty()).unwrap_or(false);
    let is_notes_sheet = s
        .doc()
        .ok()
        .map(|d| {
            let texts = cadaddy_mepfix::sheet_facts::texts(d);
            cadaddy_mepfix::sheet_facts::identify_sheet(&texts).is_some_and(|ident| {
                let up = ident.title.to_ascii_uppercase();
                // Notes/legend/schedule/section/detail sheets have no plan-view MEP geometry.
                up.contains("NOTES")
                    || up.contains("LEGEND")
                    || up.contains("ABBREVIATION")
                    || up.contains("SCHEDULE")
                    || up.contains("SYMBOLS")
                    || up.contains("SECTION")
                    || up.contains("DETAIL")
            })
        })
        .unwrap_or(false);
    if (disc == "mep" || disc == "all") && has_drawing_entities && !spec_has_mep && !is_notes_sheet {
        // First, try standard layer-based reading (mep.read {apply}).
        if let Err(e) = s.execute("mep.read", &json!({"apply": true})) {
            mep_warnings.push(format!("mep.read: {e}"));
        }
        // Then, geometry-based reading (ducts.read {apply}) for PDF-imported sheets.
        match super::readers::read_mech_from_drawing(s) {
            Ok(net) => {
                let spec_before = current(s, "bim.build").map(|(sp, _)| sp.mep.runs.len()).unwrap_or(0);
                if !net.runs.is_empty() || !net.equipment.is_empty() || !net.terminals.is_empty() {
                    let _ = s.execute("ducts.read", &json!({"apply": true}));
                }
                let spec_after = current(s, "bim.build").map(|(sp, _)| sp.mep.runs.len()).unwrap_or(0);
                if spec_after == 0 && spec_before == 0 {
                    mep_warnings.push(
                        "no MEP runs found in the drawing — mechanical sheets may need to be imported first, or duct lines may not be present"
                            .to_string(),
                    );
                }
            }
            Err(_) => {
                // mep.read already ran; proceed with whatever it found.
            }
        }
    } else if (disc == "mep" || disc == "all") && !spec_has_mep {
        // Nothing read and nothing pre-set: the MEP build comes out empty — say why.
        mep_warnings.push(if !has_drawing_entities {
            "no MEP runs: the drawing is empty — import the mechanical and plumbing sheets first, or set runs with bspec.set".to_string()
        } else {
            "no MEP runs: this sheet is a notes, legend, schedule, section or detail page — MEP is read from the plan sheets".to_string()
        });
    }

    let r = s.execute("model.build", &json!({}))?;
    let counts = built_counts(s.doc()?);
    let mut st = load(s.doc()?);
    let spec = spec_hash(s.doc()?);
    let unchanged = st.last_build == counts && st.last_build_spec == spec;
    if !unchanged {
        st.revision += 1;
        st.last_build = counts.clone();
        st.last_build_spec = spec;
        log(&mut st, "agent", "build", "", &format!("built {} level(s)", counts.len()), None);
        save(s, &mut st)?;
    }
    let pick = |m: &BTreeMap<String, usize>| -> Value { if disc == "all" { json!(m) } else { json!({ disc: m.get(disc).copied().unwrap_or(0) }) } };
    let built: Value = match level {
        Some(l) => pick(&counts.get(l).cloned().unwrap_or_default()),
        None => json!(counts.iter().map(|(l, m)| (l.clone(), pick(m))).collect::<BTreeMap<_, _>>()),
    };
    let mut warnings: Vec<Value> = r["warnings"].as_array().into_iter().flatten().take(10).cloned().collect();
    warnings.extend(mep_warnings.into_iter().map(Value::from));
    let mut out = json!({"level": level, "built": built, "unchanged": unchanged, "warnings": warnings});
    if let Some(d) = drafted {
        out["drafted"] = d;
    }
    Ok(capped(out, "warnings", ANSWER_MAX))
}

// ---- Re-apply decisions after a plan re-read -------------------------------------------------------

/// Re-apply resolved mep_guess decisions to the current runs.  Called after `mep.read {apply}`
/// and `model.build` so that cited answers survive the replacement of runs in the spec.
/// Decisions whose runs are gone become orphaned (never left "resolved" with no matching run).
pub fn reapply_decisions(s: &mut Session) {
    let mut st = load(match s.doc() {
        Ok(d) => d,
        Err(_) => return,
    });
    let resolved_mep: Vec<(String, Value, Option<Citation>)> = st
        .decisions
        .iter()
        .filter(|(id, d)| d.status == "resolved" && id.starts_with("RV-M"))
        .map(|(id, d)| (id.clone(), d.answer.clone(), d.citation.clone()))
        .collect();
    if resolved_mep.is_empty() {
        return;
    }
    // Current mep_guess items with their ids (after re-read, reviewed runs dropped back to guesses).
    let current = items(s);
    let mep_items: Vec<&Item> = current.iter().filter(|i| i.kind == "mep_guess").collect();
    let current_ids: Vec<&str> = mep_items.iter().map(|i| i.id.as_str()).collect();
    let mut changed = false;
    for (id, answer, _cite) in &resolved_mep {
        if let Some(item) = mep_items.iter().find(|i| i.id == *id) {
            // The run still exists: re-apply the saved answer.
            let mut params = json!({"id": item.key, "reviewed": true});
            if let Some(o) = params.as_object_mut() {
                if let Some(v) = answer.get("size_in").filter(|v| !v.is_null()) {
                    o.insert("size_in".into(), v.clone());
                }
                if let Some(v) = answer.get("bottom_ft").filter(|v| !v.is_null()) {
                    o.insert("bottom_ft".into(), v.clone());
                }
                if let Some(v) = answer.get("system").filter(|v| !v.is_null()) {
                    o.insert("system".into(), v.clone());
                }
            }
            let _ = s.execute("mep.edit", &params);
        } else if !current_ids.contains(&id.as_str()) {
            // The run is gone: orphan the decision so it is never left "resolved" with no run.
            if let Some(d) = st.decisions.get_mut(id) {
                let author = d.author.clone();
                let title = d.title.clone();
                let cite = d.citation.clone();
                d.status = "orphaned".into();
                st.revision += 1;
                log(&mut st, &author, "orphaned", id, &format!("the run is gone from the plans: {title}"), cite.as_ref());
                changed = true;
            }
        }
    }
    if changed {
        let _ = save(s, &mut st);
    }
}
