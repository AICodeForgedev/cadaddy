//! The review list ranked by what each item costs (Q6197): the job's findings and open
//! questions, the RFIs, Validate's fail and unsure rows, G-COVERAGE's fails, an agent's judged
//! Validate findings and the work CADaddy proposed itself (laid-out runs, placed gear) — one list,
//! each item with its dollar and labor-hour impact, sorted by how far it can move the estimate.
//!
//! - **Impact** comes from the takeoff lines an item touches, priced as the estimate prices them
//!   (`estimate`'s prices, the price book, the starter book): each element's share of every line
//!   that lists it (a device's own line and its plate's); a quantity the model doesn't have (a
//!   symbol on the plans with no element) priced like the lines of its kind; a proposed run by
//!   the foot like the lines of its size. Labor is in hours and in dollars (the estimate's crew
//!   rate, else the rules data's). An item that touches no priced line says its impact is
//!   unknown, never zero.
//! - **The confidence range**: the priced lines' total, low and high, moved by every unresolved
//!   item by its kind's fractions in the rules data (`review_impact.json`: a missing item may add
//!   its whole price, an extra one take it away, a conflict either side's price…). An element
//!   several items touch counts once, at its widest swing.
//! - **Resolving** (`accept`, `change`, `rfi_sent`; `reopen` undoes) is kept on the item in the
//!   drawing with its author and note; the list re-ranks on the next call and every decision stays
//!   in the audit trail (`review.impact.trail`). An RFI marked sent is set sent on the RFI list too.
//! - **Opening**: each item says where it is on its sheet and which elements it names;
//!   `review.impact.open {id, what: sheet|element}` zooms there (and selects the elements).
//!
//! Ids are stable (`RI-<source letter><hash>` of what the item is about), so a decision follows
//! its item from one call to the next. Validate rows of the same rule, verdict, kind and wording
//! (numbers aside) are one item with a count: a thousand receptacles with no product data are one
//! decision, not a thousand.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_walk3d::BuildingSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::agentready::ANSWER_MAX;
use super::bspec::current;
use super::*;
use crate::{Result, Session};

pub const CMD: &str = "review.impact";
const RESOLVE: &str = "review.impact.resolve";
const OPEN: &str = "review.impact.open";
const TRAIL: &str = "review.impact.trail";
/// Where the decisions live in the drawing's agent state.
const STATE_KEY: &str = "review_impact";
const RULES_JSON: &str = include_str!("review_impact.json");
const PAGE_DEFAULT: usize = 20;
const PAGE_MAX: usize = 50;
/// Items gathered at most (each source is capped too).
const MAX_ITEMS: usize = 50_000;
/// Elements an item keeps at most (to price and to open).
const MAX_HANDLES: usize = 2_000;
/// Elements an item lists in an answer at most.
const SHOWN_HANDLES: usize = 5;
/// Takeoff lines an item names at most.
const SHOWN_LINES: usize = 5;
const TITLE_MAX: usize = 160;
const NOTE_MAX: usize = 1_000;
const TRAIL_KEPT: usize = 5_000;
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// A quantity priced at most (hostile counts).
const MAX_QTY: f64 = 1e7;
/// The plan window round an item's point, feet each way.
const WINDOW_FT: f64 = 10.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Review by Impact", run_list)
            .params("{status?: open|resolved|all (open), source?: finding|question|rfi|validate|coverage|judged|proposed, level?, cursor?: n, limit?: 1-50 (20), prescreen?: bool (true: Validate's rows; false leaves them out, faster)} → {total, open, resolved, by_source, estimate: {base, low, high, labor_hours: {base, low, high}, unresolved, unknown_impact, rate, how}, effects: {effect: {low, high, why}}, items: [{rank, id, source, rule, verdict, effect, title, level, count, status, impact: {dollars, material, labor_hours, labor_dollars, known, basis, lines, unpriced?}, range: {low, high}, open: {sheet?: {sheet, at} | {rfi}, element?: {handles, count}}, resolution?}], next_cursor?, how}: every review item ranked by how far it can move the estimate, unknown impacts after the priced ones")
            .noundo(),
        CommandSpec::new(RESOLVE, "Resolve Review Item by Impact", run_resolve).params(
            "{id, action: accept|change|rfi_sent|reopen, note?: text, author?: agent|user (agent)} → {id, action, status, title, impact, range, changed, estimate_moves: bool}: the decision is kept on the item and in the audit trail; the list re-ranks on its next call (an RFI marked sent is set sent on the RFI list too)",
        ),
        CommandSpec::new(OPEN, "Open Review Item", run_open)
            .params("{id, what?: sheet|element (sheet), member?: n (one of the item's elements, from 0)} → {id, what, sheet?, at?, window?, handles?}: zooms to the item's source on its sheet, or to its elements and selects them")
            .noundo(),
        CommandSpec::new(TRAIL, "Review Audit Trail", run_trail)
            .params("{id?, cursor?: n, limit?: 1-50 (20)} → {total, entries: [{seq, id, action, before, note, author, title, source, dollars, low, high}], next_cursor?}: every decision on the review list, newest first; nothing is removed")
            .noundo(),
    ]
}

// ---------------------------------------------------------------- rules data

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Effect {
    low: f64,
    high: f64,
    why: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Action {
    in_range: bool,
    why: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    effects: BTreeMap<String, Effect>,
    verdicts: BTreeMap<String, String>,
    rules: BTreeMap<String, BTreeMap<String, String>>,
    rfi_kinds: BTreeMap<String, String>,
    finding: String,
    question: String,
    proposed: String,
    actions: BTreeMap<String, Action>,
    labor_rate: f64,
    labor_rate_source: String,
    like_categories: Vec<String>,
    run_categories: Vec<String>,
    run_item: String,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES_JSON).unwrap_or_default())
}

impl Rules {
    /// An effect's fractions, finite and within ±10 (none: no move).
    fn fractions(&self, effect: &str) -> (f64, f64) {
        let f = |v: f64| if v.is_finite() { v.clamp(-10.0, 10.0) } else { 0.0 };
        self.effects.get(effect).map_or((0.0, 0.0), |e| (f(e.low.min(e.high)), f(e.high.max(e.low))))
    }
    fn why(&self, effect: &str) -> &str {
        self.effects.get(effect).map_or("", |e| e.why.as_str())
    }
    /// A Validate row's effect: the rule's own word for its verdict, else the verdict's.
    fn of_verdict(&self, rule: &str, verdict: &str) -> String {
        self.rules.get(rule).and_then(|m| m.get(verdict)).or_else(|| self.verdicts.get(verdict)).cloned().unwrap_or_else(|| "unsure".to_string())
    }
    fn of_rfi(&self, kind: &str) -> String {
        self.rfi_kinds.get(kind).or_else(|| self.rfi_kinds.get("*")).cloned().unwrap_or_else(|| "gap".to_string())
    }
    fn in_range(&self, action: &str) -> bool {
        self.actions.get(action).is_some_and(|a| a.in_range)
    }
    fn rate(&self) -> f64 {
        if self.labor_rate.is_finite() && self.labor_rate > 0.0 { self.labor_rate.min(10_000.0) } else { 0.0 }
    }
}

// ---------------------------------------------------------------- small helpers

fn cut(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

fn r2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

fn hash8(s: &str) -> String {
    super::validate_run::hash8(s)
}

/// An issue's words with every number as `#` (rows that differ only in numbers group).
fn wording(s: &str) -> String {
    let mut out = String::new();
    let mut in_num = false;
    for c in s.chars().take(400) {
        if c.is_ascii_digit() || (in_num && c == '.') {
            if !in_num {
                out.push('#');
            }
            in_num = true;
        } else {
            in_num = false;
            out.push(c);
        }
    }
    cut(&out, 160)
}

/// Lower case, words joined by `_`.
fn words_of(s: &str) -> String {
    let t: String = s.trim().chars().take(200).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    t.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

/// Does `hay` hold the words of `needle` in a row?
fn has_words(hay: &str, needle: &str) -> bool {
    let (h, n) = (words_of(hay), words_of(needle));
    !n.is_empty() && format!("_{h}_").contains(&format!("_{n}_"))
}

fn sev_rank(s: &str) -> u8 {
    match s.trim().to_ascii_lowercase().as_str() {
        "high" | "critical" => 0,
        "medium" => 1,
        _ => 2,
    }
}

fn pt_of(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    let (x, y) = (a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

fn page(p: &Value) -> (usize, usize) {
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE_DEFAULT, |n| (n.min(1_000) as usize).clamp(1, PAGE_MAX));
    let cursor = match p.get("cursor") {
        Some(Value::String(c)) => c.trim_start_matches('c').parse::<usize>().unwrap_or(0),
        Some(v) => v.as_u64().map_or(0, |n| n.min(10_000_000) as usize),
        None => 0,
    };
    (cursor.min(10_000_000), limit)
}

/// Keep an answer under the cap: drop list entries from the end and say so.
fn capped(mut v: Value, list: &str) -> Value {
    let mut truncated = false;
    while v.to_string().len() > ANSWER_MAX {
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

// ---------------------------------------------------------------- items

/// Where an item is on its sheet: the sheet's number (when one holds the point) and the point in
/// drawing units.
#[derive(Clone, Debug, Default)]
struct SheetAt {
    sheet: String,
    at: [f64; 2],
}

/// A quantity the model doesn't have, priced like the lines of its kind.
#[derive(Clone, Debug, Default)]
struct Like {
    kind: String,
    mark: String,
    qty: f64,
}

#[derive(Clone, Debug, Default)]
struct Item {
    id: String,
    /// finding, question, rfi, validate, coverage, judged, proposed.
    source: &'static str,
    rule: String,
    verdict: String,
    effect: String,
    title: String,
    level: String,
    severity: u8,
    count: usize,
    /// The elements it names: hex handles or spec tag ids.
    handles: Vec<String>,
    sheet: Option<SheetAt>,
    /// The RFI it is (rfi.zoom, rfi.set).
    rfi: Option<String>,
    like: Option<Like>,
    /// A proposed run: its trade size (`3/4"`) and feet.
    run: Option<(String, f64)>,
    /// The share of its elements' price at stake (a count over by 2 of 10: 0.2).
    share: f64,
    /// Settled where it came from (an RFI marked sent: `rfi_sent`).
    source_status: Option<&'static str>,
}

impl Item {
    fn new(source: &'static str, key: &str) -> Item {
        let letter = match source {
            "finding" => 'F',
            "question" => 'Q',
            "rfi" => 'R',
            "validate" => 'V',
            "coverage" => 'C',
            "judged" => 'J',
            _ => 'P',
        };
        Item { id: format!("RI-{letter}{}", hash8(&format!("{source}|{key}"))), source, count: 1, share: 1.0, ..Default::default() }
    }
    fn add_handle(&mut self, h: &str) {
        let h = h.trim();
        if !h.is_empty() && self.handles.len() < MAX_HANDLES && !self.handles.iter().any(|x| x.eq_ignore_ascii_case(h)) {
            self.handles.push(h.to_string());
        }
    }
}

/// The source a review id comes from (its letter), so one item is found without gathering all.
fn source_of_id(id: &str) -> Option<&'static str> {
    match id.strip_prefix("RI-")?.chars().next()? {
        'F' => Some("finding"),
        'Q' => Some("question"),
        'R' => Some("rfi"),
        'V' => Some("validate"),
        'C' => Some("coverage"),
        'J' => Some("judged"),
        'P' => Some("proposed"),
        _ => None,
    }
}

/// What the drawing places where: its sheets' rectangles (the spec's, then the job's regions).
struct Places {
    sheets: Vec<(String, [f64; 4])>,
    frame: super::validate::Frame,
}

impl Places {
    fn of(d: &Drawing, sp: &BuildingSpec, agent: &Value) -> Places {
        let mut sheets: Vec<(String, [f64; 4])> = sp.sheets.iter().filter_map(|s| Some((s.number.clone(), s.rect?))).take(10_000).collect();
        for r in agent["guided_job"]["sheet_regions"].as_array().into_iter().flatten().take(10_000) {
            let (Some(n), Some(rect)) = (r["number"].as_str(), r["rect"].as_array()) else { continue };
            let v: Vec<f64> = rect.iter().filter_map(Value::as_f64).collect();
            if let [a, b, c, e] = v[..]
                && !sheets.iter().any(|(m, _)| m.eq_ignore_ascii_case(n))
            {
                sheets.push((n.to_string(), [a, b, c, e]));
            }
        }
        Places { sheets, frame: super::validate::Frame::of_spec(d, sp) }
    }
    /// The smallest sheet whose rectangle holds a drawing point.
    fn sheet_at(&self, p: [f64; 2]) -> String {
        self.sheets
            .iter()
            .filter(|(_, r)| p[0] >= r[0].min(r[2]) && p[0] <= r[0].max(r[2]) && p[1] >= r[1].min(r[3]) && p[1] <= r[1].max(r[3]))
            .min_by(|a, b| ((a.1[2] - a.1[0]) * (a.1[3] - a.1[1])).abs().total_cmp(&((b.1[2] - b.1[0]) * (b.1[3] - b.1[1])).abs()))
            .map_or_else(String::new, |(n, _)| n.clone())
    }
    fn at_drawing(&self, sheet: &str, p: [f64; 2]) -> SheetAt {
        let found = self.sheet_at(p);
        SheetAt { sheet: if found.is_empty() { sheet.to_string() } else { found }, at: [r2(p[0]), r2(p[1])] }
    }
    /// A building point (feet) on a level's plan, on its sheet.
    fn at_level(&self, level: &str, ft: [f64; 2]) -> SheetAt {
        let q = self.frame.to_sheet(level, ft);
        self.at_drawing("", [q.x, q.y])
    }
}

fn agent_json(d: &Drawing) -> Value {
    d.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// The job's findings and its open questions (read field by field: a job saved by another
/// version still lists).
fn job_items(agent: &Value, places: &Places, out: &mut Vec<Item>) {
    let r = rules();
    let job = &agent["guided_job"];
    let status: HashMap<&str, &str> = job["worklist"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .filter_map(|w| Some((w["id"].as_str()?, w["status"].as_str().unwrap_or("pending"))))
        .collect();
    let evidence = |it: &mut Item, ev: &Value| {
        for h in ev["handles"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            it.add_handle(h);
        }
        if let Some(at) = pt_of(&ev["at"]) {
            it.sheet = Some(places.at_drawing(ev["sheet"].as_str().unwrap_or_default(), at));
        }
    };
    for f in job["findings"].as_array().into_iter().flatten().take(MAX_ITEMS) {
        let id = f["id"].as_str().unwrap_or_default();
        if status.get(id).is_some_and(|s| *s == "resolved") {
            continue;
        }
        let what = f["description"].as_str().unwrap_or_default();
        let mut it = Item::new("finding", &format!("{id}|{what}"));
        it.rule = f["stage"].as_str().unwrap_or_default().to_string();
        it.verdict = "finding".into();
        it.effect = r.finding.clone();
        it.title = cut(&format!("{id}: {what}"), TITLE_MAX);
        it.severity = match f["severity"].as_u64().unwrap_or(1) {
            4.. => 0,
            3 => 1,
            _ => 2,
        };
        evidence(&mut it, &f["evidence"]);
        out.push(it);
    }
    for w in job["worklist"].as_array().into_iter().flatten().take(MAX_ITEMS) {
        if w["kind"] != "question" || !matches!(w["status"].as_str(), Some("pending") | None) {
            continue;
        }
        let id = w["id"].as_str().unwrap_or_default();
        let what = w["summary"].as_str().unwrap_or_default();
        let mut it = Item::new("question", &format!("{id}|{what}"));
        it.rule = w["stage"].as_str().unwrap_or_default().to_string();
        it.verdict = "question".into();
        it.effect = r.question.clone();
        it.title = cut(&format!("{id}: {what}"), TITLE_MAX);
        it.severity = 1;
        evidence(&mut it, &w["evidence"]);
        out.push(it);
    }
}

/// The RFIs still open or sent (answered, closed, approved and rejected ones are settled).
fn rfi_items(sp: &BuildingSpec, places: &Places, out: &mut Vec<Item>) {
    let r = rules();
    for x in cadaddy_walk3d::rfi::list(sp).into_iter().take(MAX_ITEMS) {
        let status = x.status.name();
        if !matches!(status, "open" | "sent") {
            continue;
        }
        let mut it = Item::new("rfi", &format!("{}|{}", x.id, x.issue));
        it.rule = x.kind_name().to_string();
        it.verdict = "rfi".into();
        it.effect = r.of_rfi(x.kind_name());
        it.title = cut(&format!("{}: {}", x.id, x.issue), TITLE_MAX);
        it.level = x.level.clone();
        it.severity = sev_rank(x.severity.name());
        for h in x.elements.iter().take(MAX_HANDLES) {
            it.add_handle(h);
        }
        if !x.level.is_empty() && x.at.iter().all(|v| v.is_finite()) && x.at != [0.0, 0.0] {
            it.sheet = Some(places.at_drawing(x.sheets.first().map_or("", String::as_str), x.at));
        }
        it.rfi = Some(x.id.clone());
        if status == "sent" {
            it.source_status = Some("rfi_sent");
        }
        out.push(it);
    }
}

/// Validate's problem rows (the prescreen over the whole building), rows that differ only in
/// numbers as one item; G-COVERAGE's each its own. A run CADaddy proposes comes from the spec
/// (proposed work), not again from its `proposed` row.
fn validate_items(rows: &[Value], places: &Places, out: &mut Vec<Item>) {
    let r = rules();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for f in rows.iter().take(MAX_ITEMS * 4) {
        let verdict = f["verdict"].as_str().unwrap_or_default();
        if matches!(verdict, "pass" | "proposed" | "") {
            continue;
        }
        let rule = f["rule"].as_str().unwrap_or_default();
        let coverage = rule == super::validate_coverage::RULE;
        let kind = f["kind"].as_str().unwrap_or_default();
        let issue = f["issue"].as_str().unwrap_or_default();
        let key = if coverage {
            format!("{rule}|{verdict}|{kind}|{}", f["id"].as_str().unwrap_or_default())
        } else {
            format!("{rule}|{verdict}|{kind}|{}", wording(issue))
        };
        let at = match groups.get(&key) {
            Some(i) => *i,
            None => {
                if out.len() >= MAX_ITEMS {
                    break;
                }
                let mut it = Item::new(if coverage { "coverage" } else { "validate" }, &key);
                it.rule = rule.to_string();
                it.verdict = verdict.to_string();
                it.effect = r.of_verdict(rule, verdict);
                it.title = cut(issue, TITLE_MAX);
                it.level = f["level"].as_str().unwrap_or_default().to_string();
                it.severity = sev_rank(f["severity"].as_str().unwrap_or_default());
                it.count = 0;
                let src = &f["expected_source"];
                it.sheet = match (pt_of(&src["at"]), pt_of(&f["at"])) {
                    (Some(p), _) => Some(places.at_drawing(src["sheet"].as_str().unwrap_or_default(), p)),
                    (None, Some(ft)) if !it.level.is_empty() => Some(places.at_level(&it.level, ft)),
                    _ => None,
                };
                out.push(it);
                groups.insert(key, out.len() - 1);
                out.len() - 1
            }
        };
        let Some(it) = out.get_mut(at) else { continue };
        it.count += 1;
        if !it.level.is_empty() && f["level"].as_str().is_some_and(|l| !l.eq_ignore_ascii_case(&it.level)) {
            it.level.clear();
        }
        if let Some(h) = f["handle"].as_str() {
            it.add_handle(h);
        }
        let m = &f["measured"];
        if coverage {
            let marks = m["plan_marks"].as_f64().unwrap_or(0.0);
            let model = m["model"].as_f64().unwrap_or(0.0);
            if marks > model {
                let mark = m["codes"].as_array().and_then(|c| c.first()).and_then(Value::as_str).unwrap_or_default();
                let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(kind);
                it.like = Some(Like { kind: name.to_string(), mark: mark.to_string(), qty: (marks - model).min(MAX_QTY) });
            } else if model > marks && model > 0.0 {
                for h in m["model_elements"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    it.add_handle(h);
                }
                it.share = ((model - marks) / model).clamp(0.0, 1.0);
            }
        } else if f["handle"].as_str().is_none() && it.effect == "missing" {
            // A symbol or a set's item with no element: its kind and how many are short.
            let (kind, mark, qty) = shortfall(f);
            let like = it.like.get_or_insert_with(|| Like { kind, mark, qty: 0.0 });
            like.qty = (like.qty + qty).min(MAX_QTY);
        }
        // Gear rows name their gear by its tag.
        if f["handle"].as_str().is_none()
            && let Some(t) = f["tag"].as_str().or_else(|| m["gear"].as_str()).filter(|t| !t.trim().is_empty())
        {
            it.add_handle(t);
        }
        for h in m["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).take(50) {
            if it.effect != "missing" {
                it.add_handle(h);
            }
        }
    }
    for it in out.iter_mut().filter(|i| matches!(i.source, "validate" | "coverage") && i.count > 1) {
        it.title = cut(&format!("{} ×{}: {}", it.rule, it.count, it.title), TITLE_MAX);
    }
    for it in out.iter_mut().filter(|i| matches!(i.source, "validate" | "coverage") && i.count <= 1 && !i.title.starts_with(&i.rule)) {
        it.title = cut(&format!("{} {}: {}", it.rule, it.verdict, it.title), TITLE_MAX);
    }
}

/// What a `missing` row is short of: its kind (the row's, else a project set's item before its
/// `=`), its mark (the row's or its symbol type) and how many (the plan's count less the model's,
/// a set's count less what the room holds, else one).
fn shortfall(f: &Value) -> (String, String, f64) {
    let m = &f["measured"];
    let item = m["item"].as_str().unwrap_or_default();
    let set_kind = item.split(['=', '@']).next().unwrap_or_default().trim();
    let kind = f["kind"].as_str().or_else(|| m["kind"].as_str()).filter(|k| !k.trim().is_empty()).unwrap_or(set_kind).to_string();
    let mark = f["mark"].as_str().or_else(|| m["mark"].as_str()).or_else(|| m["type"].as_str()).unwrap_or_default().to_string();
    let n = |v: &Value| v.as_f64().filter(|x| x.is_finite());
    let set_count = item.split_once('=').and_then(|(_, c)| c.trim().trim_end_matches('+').parse::<f64>().ok()).filter(|c| c.is_finite());
    let qty = match (n(&m["plan"]), n(&m["model"]), set_count, n(&m["count"])) {
        (Some(plan), Some(model), _, _) if plan > model => plan - model,
        (_, _, Some(want), Some(have)) if want > have => want - have,
        _ => 1.0,
    };
    (kind, mark, qty.clamp(0.0, MAX_QTY))
}

/// The findings an agent judged in a Validate run (not passes).
fn judged_items(d: &Drawing, places: &Places, out: &mut Vec<Item>) {
    let r = rules();
    for j in super::validate_run::judge_items(d).into_iter().take(MAX_ITEMS) {
        let mut it = Item::new("judged", &j.id);
        it.rule = j.detail["rule"].as_str().unwrap_or_default().to_string();
        it.verdict = j.detail["verdict"].as_str().unwrap_or_default().to_string();
        it.effect = r.of_verdict(&it.rule, &it.verdict);
        it.title = cut(&j.title, TITLE_MAX);
        it.level = j.level.clone();
        it.severity = sev_rank(j.detail["severity"].as_str().unwrap_or_default());
        for h in j.detail["handles"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            it.add_handle(h);
        }
        if let Some(b) = j.detail["zoom"]["box_ft"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
            && let [x0, y0, x1, y1] = b[..]
        {
            it.sheet = Some(places.at_level(&j.level, [(x0 + x1) / 2.0, (y0 + y1) / 2.0]));
        }
        out.push(it);
    }
}

/// The work CADaddy laid out itself: gear it placed (`sources.status: proposed`) and runs it
/// routed (`height_source: proposed`), the runs by system, level and size.
fn proposed_items(sp: &BuildingSpec, places: &Places, out: &mut Vec<Item>) {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    for e in sp.mep.equipment.iter().take(MAX_ITEMS) {
        if !e.sources.get("status").is_some_and(|s| s.eq_ignore_ascii_case("proposed")) {
            continue;
        }
        let mut it = Item::new("proposed", &format!("equipment|{}|{}", e.tag, e.level));
        it.rule = "equipment".into();
        it.verdict = "proposed".into();
        it.effect = r.proposed.clone();
        let place = e.sources.get("position").map_or_else(String::new, |p| format!(" — {p}"));
        it.title = cut(&format!("{} {} placed by CADaddy{place}", e.kind, e.tag), TITLE_MAX);
        it.level = e.level.clone();
        it.severity = 1;
        it.add_handle(&e.tag);
        let c = [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0];
        if c.iter().all(|v| v.is_finite()) {
            it.sheet = Some(places.at_drawing("", c));
        }
        out.push(it);
    }
    let mut runs: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for run in sp.mep.runs.iter().take(MAX_ITEMS * 4) {
        if !run.height_source.eq_ignore_ascii_case("proposed") {
            continue;
        }
        let size = super::mepatoms::trade_size(run.size_in[0]);
        let plan: f64 = run
            .points
            .windows(2)
            .filter_map(|w| if let [a, b] = w { Some((b[0] - a[0]).hypot(b[1] - a[1]) / upf) } else { None })
            .filter(|v| v.is_finite())
            .sum();
        let rise = match (run.top_ft, run.bottoms_ft.first()) {
            (Some(t), Some(b)) if run.points.len() <= 1 => (t - b).abs(),
            _ => 0.0,
        };
        let ft = (plan + if rise.is_finite() { rise } else { 0.0 }).min(MAX_QTY);
        let key = (run.system.clone(), run.level.clone(), size.clone());
        let at = match runs.get(&key) {
            Some(i) => *i,
            None => {
                if out.len() >= MAX_ITEMS {
                    break;
                }
                let mut it = Item::new("proposed", &format!("run|{}|{}|{size}", run.system, run.level));
                it.rule = run.system.clone();
                it.verdict = "proposed".into();
                it.effect = r.proposed.clone();
                it.level = run.level.clone();
                it.severity = 1;
                it.count = 0;
                it.run = Some((size.clone(), 0.0));
                if let Some(p) = run.points.first().copied().filter(|p| p.iter().all(|v| v.is_finite())) {
                    it.sheet = Some(places.at_drawing(&run.source_sheet, p));
                }
                out.push(it);
                runs.insert(key, out.len() - 1);
                out.len() - 1
            }
        };
        let Some(it) = out.get_mut(at) else { continue };
        it.count += 1;
        if let Some((_, f)) = it.run.as_mut() {
            *f = (*f + ft).min(MAX_QTY);
        }
        for o in run.objects.iter().take(20) {
            it.add_handle(o);
        }
    }
    for it in out.iter_mut().filter(|i| i.source == "proposed" && i.run.is_some()) {
        let (size, ft) = it.run.clone().unwrap_or_default();
        it.title = cut(&format!("{} {size} laid out by CADaddy on {}: {} run(s), {:.0} ft", it.rule, it.level, it.count, ft), TITLE_MAX);
    }
}

/// Every item of the sources asked for (all when `only` is empty).
fn gather(s: &mut Session, only: &[&str], prescreen: bool) -> Result<(Vec<Item>, Places)> {
    let (sp, _) = current(s, CMD)?;
    let doc = std::sync::Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let agent = agent_json(d);
    let places = Places::of(d, &sp, &agent);
    let want = |x: &str| only.is_empty() || only.contains(&x);
    let mut out = Vec::new();
    if want("finding") || want("question") {
        job_items(&agent, &places, &mut out);
        out.retain(|i| want(i.source));
    }
    if want("rfi") {
        rfi_items(&sp, &places, &mut out);
    }
    if prescreen && (want("validate") || want("coverage")) {
        let screened = super::validate_prescreen::screen_session(s, &json!({}), &[])?;
        let start = out.len();
        validate_items(&screened.out.findings, &places, &mut out);
        let mut i = 0;
        out.retain(|it| {
            i += 1;
            i <= start || want(it.source)
        });
    }
    if want("judged") {
        judged_items(d, &places, &mut out);
    }
    if want("proposed") {
        proposed_items(&sp, &places, &mut out);
    }
    out.truncate(MAX_ITEMS);
    Ok((out, places))
}

// ---------------------------------------------------------------- prices

/// A price per element or per unit: material, labor hours and dollars, the lines it is from.
#[derive(Clone, Debug, Default)]
struct Unit {
    material: f64,
    hours: f64,
    labor: f64,
    dollars: f64,
    lines: Vec<String>,
}

impl Unit {
    fn add(&mut self, o: &Unit, times: f64) {
        self.material += o.material * times;
        self.hours += o.hours * times;
        self.labor += o.labor * times;
        self.dollars += o.dollars * times;
        for l in &o.lines {
            if self.lines.len() < SHOWN_LINES && !self.lines.contains(l) {
                self.lines.push(l.clone());
            }
        }
    }
}

/// A priced takeoff line: its words and its price per unit of quantity.
struct LineP {
    category: String,
    item: String,
    size: String,
    unit: String,
    net: f64,
    per: Unit,
}

/// The takeoff priced: per element key and per line, and the total.
struct Prices {
    rate: f64,
    rate_from: String,
    per_key: HashMap<String, Unit>,
    /// Keys only unpriced lines list.
    unpriced: HashSet<String>,
    lines: Vec<LineP>,
    base: Unit,
    priced_lines: usize,
    unpriced_lines: usize,
    st: Map<String, Value>,
}

/// An element's keys a takeoff line may list it by: its handle and its spec tag id (specific),
/// then its tag and name (shared by every element of that tag).
fn keys_of(e: &Entity) -> (Vec<String>, Vec<String>) {
    let mut specific = vec![e.handle.hex().to_ascii_uppercase()];
    if let Some(t) = super::bimlook::made_tag(e, "devid").filter(|t| !t.is_empty()) {
        specific.push(t.to_ascii_uppercase());
    }
    let mut shared = Vec::new();
    for t in [super::bimlook::made_tag(e, "tag"), super::bimlook::made_name(e)].into_iter().flatten() {
        let t = t.trim().to_ascii_uppercase();
        if !t.is_empty() && !shared.contains(&t) {
            shared.push(t);
        }
    }
    (specific, shared)
}

fn line_label(l: &Value) -> String {
    let item = l["item"].as_str().unwrap_or_default();
    let size = l["size"].as_str().unwrap_or_default();
    cut(&if size.is_empty() { item.to_string() } else { format!("{item} {size}") }, 80)
}

fn prices(s: &mut Session, d: &Drawing) -> Result<Prices> {
    let r = rules();
    let (_, st) = super::estimate::state(s, CMD)?;
    let bom = s.execute("bim.bom", &json!({"all_elements": true}))?;
    let (priced, _) = super::estimate::price_lines(&st, &bom);
    let crew = super::estimate::setting(&st, "crew_rate", 0.0);
    let (rate, rate_from) = if crew.is_finite() && crew > 0.0 {
        (crew, "the estimate's crew rate (estimate.settings crew_rate)".to_string())
    } else {
        (r.rate(), r.labor_rate_source.clone())
    };
    // How many elements carry each shared key (a tag every exhaust fan has).
    let mut carriers: HashMap<String, f64> = HashMap::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        let (specific, shared) = keys_of(e);
        for k in specific.into_iter().skip(1).chain(shared) {
            *carriers.entry(k).or_default() += 1.0;
        }
    }
    let mut sums: HashMap<String, (Unit, f64)> = HashMap::new();
    let mut listed_unpriced: HashSet<String> = HashSet::new();
    let mut lines = Vec::new();
    let mut base = Unit::default();
    let (mut priced_lines, mut unpriced_lines) = (0, 0);
    for p in &priced {
        let not_ours = p.scope.as_deref().is_some_and(|sc| matches!(sc, "by_others" | "nic" | "ofci"));
        let ok = !p.match_kind.is_empty();
        let labor = if crew > 0.0 { p.labor_cost } else { p.labor_hours * rate };
        let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
        let mut whole = Unit {
            material: fin(p.material),
            hours: fin(p.labor_hours),
            labor: fin(labor),
            dollars: fin(p.material) + fin(labor),
            lines: vec![format!("{}{}", line_label(&p.line), if not_ours { " (not ours)" } else { "" })],
        };
        if not_ours {
            whole.material = 0.0;
            whole.hours = 0.0;
            whole.labor = 0.0;
            whole.dollars = 0.0;
        }
        if ok {
            priced_lines += 1;
            base.add(&whole, 1.0);
        } else {
            unpriced_lines += 1;
        }
        let elements: Vec<String> =
            p.line["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_ascii_uppercase).collect();
        let net = if p.net.is_finite() && p.net > 0.0 { p.net } else { p.qty };
        if ok && net.is_finite() && net > 0.0 {
            let mut per = whole.clone();
            per.material /= net;
            per.hours /= net;
            per.labor /= net;
            per.dollars /= net;
            lines.push(LineP {
                category: p.line["category"].as_str().unwrap_or_default().to_ascii_lowercase(),
                item: p.line["item"].as_str().unwrap_or_default().to_string(),
                size: p.line["size"].as_str().unwrap_or_default().to_string(),
                unit: p.unit.to_ascii_lowercase(),
                net,
                per,
            });
        }
        if elements.is_empty() {
            continue;
        }
        if !ok {
            listed_unpriced.extend(elements);
            continue;
        }
        let n = elements.len() as f64;
        let mut count: HashMap<&str, f64> = HashMap::new();
        for k in &elements {
            *count.entry(k.as_str()).or_default() += 1.0;
        }
        for (k, c) in count {
            let e = sums.entry(k.to_string()).or_default();
            e.0.add(&whole, c / n);
            e.1 = e.1.max(c);
        }
    }
    let per_key: HashMap<String, Unit> = sums
        .into_iter()
        .map(|(k, (mut u, most))| {
            let by = carriers.get(&k).copied().unwrap_or(0.0).max(most).max(1.0);
            u.material /= by;
            u.hours /= by;
            u.labor /= by;
            u.dollars /= by;
            (k, u)
        })
        .collect();
    let unpriced = listed_unpriced.into_iter().filter(|k| !per_key.contains_key(k)).collect();
    Ok(Prices { rate, rate_from, per_key, unpriced, lines, base, priced_lines, unpriced_lines, st })
}

/// What an item can move the estimate by.
#[derive(Clone, Debug, Default)]
struct Impact {
    unit: Unit,
    known: bool,
    basis: Vec<String>,
    /// Elements no priced line lists (or lines with no price).
    unpriced: usize,
    /// Per element: (key it counts once by, dollars, hours).
    parts: Vec<(String, f64, f64)>,
    /// Priced as a quantity (no elements to count once): dollars, hours.
    loose: (f64, f64),
}

/// The built elements made from each plan object (a plan symbol's device box).
fn twins(d: &Drawing) -> HashMap<String, Vec<Handle>> {
    let mut m: HashMap<String, Vec<Handle>> = HashMap::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        if let Some(src) = super::bimlook::made_source(e) {
            let v = m.entry(src.hex().to_ascii_uppercase()).or_default();
            if v.len() < 16 {
                v.push(e.handle);
            }
        }
    }
    m
}

struct Pricer<'a> {
    d: &'a Drawing,
    p: &'a Prices,
    twins: HashMap<String, Vec<Handle>>,
}

impl Pricer<'_> {
    /// One named element's price and the key it counts once by.
    fn element(&self, h: &str) -> Option<(String, Unit)> {
        let up = h.trim().to_ascii_uppercase();
        let entity = Handle::parse_hex(&up).and_then(|x| self.d.model.get(x));
        let Some(e) = entity else {
            return self.p.per_key.get(&up).map(|u| (up.clone(), u.clone()));
        };
        let mut ents: Vec<&Entity> = vec![e.as_ref()];
        for t in self.twins.get(&up).into_iter().flatten() {
            if let Some(x) = self.d.model.get(*t) {
                ents.push(x.as_ref());
            }
        }
        let mut u = Unit::default();
        let mut any = false;
        for e in &ents {
            let (specific, shared) = keys_of(e);
            let hits: Vec<&Unit> = specific.iter().filter_map(|k| self.p.per_key.get(k)).collect();
            let hits = if hits.is_empty() { shared.iter().filter_map(|k| self.p.per_key.get(k)).take(1).collect() } else { hits };
            for x in hits {
                u.add(x, 1.0);
                any = true;
            }
        }
        any.then_some((up, u))
    }

    fn listed_unpriced(&self, h: &str) -> bool {
        let up = h.trim().to_ascii_uppercase();
        if self.p.unpriced.contains(&up) {
            return true;
        }
        Handle::parse_hex(&up).and_then(|x| self.d.model.get(x)).is_some_and(|e| {
            let (a, b) = keys_of(e);
            a.iter().chain(b.iter()).any(|k| self.p.unpriced.contains(k))
        })
    }

    /// A missing quantity priced like the lines of its kind (its mark the last word of the line's
    /// item, or the kind's words in it), a category at a time; else as the price book prices the
    /// kind's words.
    fn like(&self, l: &Like) -> Option<Unit> {
        let r = rules();
        let words = l.kind.replace('_', " ").to_ascii_lowercase();
        let mark = l.mark.trim().to_ascii_lowercase();
        let mut by_cat: BTreeMap<&str, (Unit, f64)> = BTreeMap::new();
        for x in &self.p.lines {
            if !r.like_categories.iter().any(|c| c.eq_ignore_ascii_case(&x.category)) || x.unit != "each" {
                continue;
            }
            let item = x.item.to_ascii_lowercase();
            let last = item.rsplit(' ').next().unwrap_or_default();
            let hit = (!mark.is_empty() && last == mark) || (!words.trim().is_empty() && has_words(&item, &words));
            if hit {
                let e = by_cat.entry(x.category.as_str()).or_default();
                e.0.add(&x.per, x.net);
                e.1 += x.net;
            }
        }
        let qty = l.qty.clamp(0.0, MAX_QTY);
        if !by_cat.is_empty() {
            let mut u = Unit::default();
            for (_, (sum, n)) in by_cat {
                if n > 0.0 {
                    u.add(&sum, qty / n);
                }
            }
            return Some(u);
        }
        if words.trim().is_empty() || qty <= 0.0 {
            return None;
        }
        let line = json!({"trade": "", "category": "", "item": words.trim(), "size": "", "unit": "each", "total": qty, "net": qty});
        let (priced, _) = super::estimate::price_lines(&self.p.st, &json!({ "lines": [line] }));
        let pr = priced.first().filter(|p| !p.match_kind.is_empty())?;
        let labor = if pr.labor_cost > 0.0 { pr.labor_cost } else { pr.labor_hours * self.p.rate };
        Some(Unit {
            material: pr.material,
            hours: pr.labor_hours,
            labor,
            dollars: pr.material + labor,
            lines: vec![format!("{} ({})", line_label(&pr.line), pr.via)],
        })
    }

    /// A proposed run priced by the foot like the takeoff's lines of its size.
    fn run(&self, size: &str, ft: f64) -> Option<Unit> {
        let r = rules();
        let mut sum = Unit::default();
        let mut n = 0.0;
        for x in &self.p.lines {
            if r.run_categories.iter().any(|c| c.eq_ignore_ascii_case(&x.category)) && x.unit == "ft" && x.size.eq_ignore_ascii_case(size) {
                sum.add(&x.per, x.net);
                n += x.net;
            }
        }
        let ft = ft.clamp(0.0, MAX_QTY);
        if n <= 0.0 {
            // No line of its size: the rules data's run item through the price book.
            let item = r.run_item.trim();
            if item.is_empty() || ft <= 0.0 {
                return None;
            }
            let line = json!({"trade": "electrical", "category": "conduit", "item": item, "size": size, "unit": "ft", "total": ft, "net": ft});
            let (priced, _) = super::estimate::price_lines(&self.p.st, &json!({ "lines": [line] }));
            let pr = priced.first().filter(|p| !p.match_kind.is_empty())?;
            let labor = if pr.labor_cost > 0.0 { pr.labor_cost } else { pr.labor_hours * self.p.rate };
            return Some(Unit {
                material: pr.material,
                hours: pr.labor_hours,
                labor,
                dollars: pr.material + labor,
                lines: vec![format!("{} ({}; no takeoff line of its size)", line_label(&pr.line), pr.via)],
            });
        }
        let mut u = Unit::default();
        u.add(&sum, ft / n);
        Some(u)
    }

    fn impact(&self, it: &Item) -> Impact {
        let mut im = Impact::default();
        let share = it.share.clamp(0.0, 1.0);
        let mut counted = 0usize;
        for h in &it.handles {
            match self.element(h) {
                Some((key, u)) => {
                    counted += 1;
                    im.parts.push((key, u.dollars * share, u.hours * share));
                    im.unit.add(&u, share);
                }
                None if self.listed_unpriced(h) => im.unpriced += 1,
                None => {}
            }
        }
        if counted > 0 {
            im.known = true;
            im.basis.push(if share < 1.0 {
                format!("{:.0}% of {counted} element(s)' share of the takeoff lines that list them", share * 100.0)
            } else {
                format!("{counted} element(s)' share of the takeoff lines that list them")
            });
        }
        if let Some(l) = &it.like {
            match self.like(l) {
                Some(u) => {
                    im.known = true;
                    im.loose.0 += u.dollars;
                    im.loose.1 += u.hours;
                    im.basis.push(format!("{} {} priced like {}", fmt_qty(l.qty), l.kind.replace('_', " "), u.lines.join(", ")));
                    im.unit.add(&u, 1.0);
                }
                None => im.basis.push(format!("{} {}: no takeoff line or price for its kind", fmt_qty(l.qty), l.kind.replace('_', " "))),
            }
        }
        if let Some((size, ft)) = &it.run {
            match self.run(size, *ft) {
                Some(u) => {
                    im.known = true;
                    im.loose.0 += u.dollars;
                    im.loose.1 += u.hours;
                    im.basis.push(format!("{ft:.0} ft priced by the foot like {}", u.lines.join(", ")));
                    im.unit.add(&u, 1.0);
                }
                None => im.basis.push(format!("{ft:.0} ft of {size}: no takeoff line or price of that size")),
            }
        }
        if !im.known && im.basis.is_empty() {
            im.basis.push(if it.handles.is_empty() {
                "names no element and no quantity".to_string()
            } else if im.unpriced > 0 {
                format!("its {} element(s) are only on unpriced takeoff lines", im.unpriced)
            } else {
                format!("no takeoff line lists its {} element(s)", it.handles.len())
            });
        }
        im
    }
}

fn fmt_qty(q: f64) -> String {
    if (q - q.round()).abs() < 1e-9 { format!("{q:.0}") } else { format!("{q:.2}") }
}

// ---------------------------------------------------------------- state

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Decision {
    action: String,
    note: String,
    author: String,
    seq: u64,
    title: String,
    source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct TrailEntry {
    seq: u64,
    id: String,
    action: String,
    /// What it was before (`open` or the earlier action).
    before: String,
    note: String,
    author: String,
    title: String,
    source: String,
    dollars: Option<f64>,
    low: Option<f64>,
    high: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    seq: u64,
    decisions: BTreeMap<String, Decision>,
    trail: Vec<TrailEntry>,
}

fn load(d: &Drawing) -> State {
    serde_json::from_value(agent_json(d)[STATE_KEY].clone()).unwrap_or_default()
}

fn save(s: &mut Session, st: &mut State) -> Result<()> {
    let n = st.trail.len();
    if n > TRAIL_KEPT {
        st.trail.drain(..n - TRAIL_KEPT);
    }
    let doc = s.doc_mut()?;
    let mut root = agent_json(doc);
    if let Some(o) = root.as_object_mut() {
        o.insert(STATE_KEY.into(), serde_json::to_value(&*st).unwrap_or(Value::Null));
    }
    doc.agent = serde_json::to_string(&root).ok();
    Ok(())
}

/// An item's status: its decision, else what its source says (an RFI marked sent), else open.
fn status_of<'a>(st: &'a State, it: &Item) -> &'a str {
    match st.decisions.get(&it.id) {
        Some(d) => d.action.as_str(),
        None => it.source_status.unwrap_or("open"),
    }
}

fn author(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    let requested = str_param(p, "author").unwrap_or("agent");
    if s.agent.from_agent {
        if requested == "user" {
            return Err(bad(cmd, "an agent's decision is tagged \"agent\" — only a person's call uses author: \"user\""));
        }
        return Ok("agent".to_string());
    }
    match requested {
        a @ ("agent" | "user") => Ok(a.to_string()),
        other => Err(bad(cmd, format!("author `{}` isn't known — use agent or user", cut(other, 20)))),
    }
}

// ---------------------------------------------------------------- ranking

struct Ranked {
    item: Item,
    impact: Impact,
    low: f64,
    high: f64,
    status: String,
}

impl Ranked {
    fn swing(&self) -> f64 {
        if self.impact.known { self.low.abs().max(self.high.abs()) } else { -1.0 }
    }
}

fn rank(items: Vec<Item>, pricer: &Pricer, st: &State) -> Vec<Ranked> {
    let r = rules();
    let mut v: Vec<Ranked> = items
        .into_iter()
        .map(|item| {
            let impact = pricer.impact(&item);
            let (lo, hi) = r.fractions(&item.effect);
            let status = status_of(st, &item).to_string();
            Ranked { low: impact.unit.dollars * lo, high: impact.unit.dollars * hi, impact, status, item }
        })
        .collect();
    v.sort_by(|a, b| {
        b.swing()
            .total_cmp(&a.swing())
            .then(a.item.severity.cmp(&b.item.severity))
            .then(b.item.count.cmp(&a.item.count))
            .then(a.item.id.cmp(&b.item.id))
    });
    v
}

/// Does an item still move the estimate (open, or settled by an action that keeps it in the
/// range)?
fn in_range(x: &Ranked) -> bool {
    x.status == "open" || rules().in_range(&x.status)
}

/// The confidence range: the priced lines' total moved by every unresolved item, an element
/// several items touch counted once at its widest swing.
fn range(all: &[Ranked], p: &Prices) -> Value {
    let r = rules();
    let mut lo_el: HashMap<&str, (f64, f64)> = HashMap::new();
    let mut hi_el: HashMap<&str, (f64, f64)> = HashMap::new();
    let (mut lo, mut hi, mut lo_h, mut hi_h) = (0.0, 0.0, 0.0, 0.0);
    let (mut unresolved, mut unknown) = (0usize, 0usize);
    for x in all.iter().filter(|x| in_range(x)) {
        unresolved += 1;
        if !x.impact.known {
            unknown += 1;
            continue;
        }
        let (fl, fh) = r.fractions(&x.item.effect);
        for (k, dollars, hours) in &x.impact.parts {
            let e = lo_el.entry(k.as_str()).or_insert((0.0, 0.0));
            *e = (e.0.min(dollars * fl), e.1.min(hours * fl));
            let e = hi_el.entry(k.as_str()).or_insert((0.0, 0.0));
            *e = (e.0.max(dollars * fh), e.1.max(hours * fh));
        }
        lo += x.impact.loose.0 * fl;
        hi += x.impact.loose.0 * fh;
        lo_h += x.impact.loose.1 * fl;
        hi_h += x.impact.loose.1 * fh;
    }
    lo += lo_el.values().map(|v| v.0).sum::<f64>();
    lo_h += lo_el.values().map(|v| v.1).sum::<f64>();
    hi += hi_el.values().map(|v| v.0).sum::<f64>();
    hi_h += hi_el.values().map(|v| v.1).sum::<f64>();
    json!({
        "base": r2(p.base.dollars),
        "low": r2((p.base.dollars + lo).max(0.0)),
        "high": r2(p.base.dollars + hi),
        "material": r2(p.base.material),
        "labor_hours": {"base": r2(p.base.hours), "low": r2((p.base.hours + lo_h).max(0.0)), "high": r2(p.base.hours + hi_h)},
        "unresolved": unresolved,
        "unknown_impact": unknown,
        "priced_lines": p.priced_lines,
        "unpriced_lines": p.unpriced_lines,
        "rate": {"dollars_per_hour": r2(p.rate), "from": p.rate_from},
        "how": "the takeoff's priced lines (material + labor hours at the rate, before tax, overhead and profit), each unresolved item moving it by its kind's fractions of its own price (rules data review_impact.json); an element several items touch counts once, at its widest swing; items of unknown impact are counted, not priced",
    })
}

fn item_json(x: &Ranked, n: usize) -> Value {
    let it = &x.item;
    let im = &x.impact;
    let mut impact = json!({
        "known": im.known,
        "basis": im.basis.join("; "),
        "lines": im.unit.lines,
    });
    if im.known {
        impact["dollars"] = json!(r2(im.unit.dollars));
        impact["material"] = json!(r2(im.unit.material));
        impact["labor_hours"] = json!(r2(im.unit.hours));
        impact["labor_dollars"] = json!(r2(im.unit.labor));
    } else {
        impact["dollars"] = json!("unknown");
        impact["labor_hours"] = json!("unknown");
    }
    if im.unpriced > 0 {
        impact["unpriced"] = json!(im.unpriced);
    }
    // Where `review.impact.open {id, what}` goes: its sheet and point, its elements.
    let mut open = json!({});
    if let Some(sh) = &it.sheet {
        open["sheet"] = json!({"sheet": sh.sheet, "at": sh.at});
    } else if let Some(id) = &it.rfi {
        open["sheet"] = json!({"rfi": id});
    }
    if !it.handles.is_empty() {
        open["element"] = json!({"handles": it.handles.iter().take(SHOWN_HANDLES).collect::<Vec<_>>(), "count": it.handles.len()});
    }
    let mut v = json!({
        "rank": n,
        "id": it.id,
        "source": it.source,
        "rule": it.rule,
        "verdict": it.verdict,
        "effect": it.effect,
        "title": it.title,
        "level": it.level,
        "count": it.count,
        "status": x.status,
        "impact": impact,
        "range": if im.known { json!({"low": r2(x.low), "high": r2(x.high)}) } else { json!({"low": "unknown", "high": "unknown"}) },
        "open": open,
    });
    if let Some(rfi) = &it.rfi {
        v["rfi"] = json!(rfi);
    }
    v
}

/// How each kind of item moves the estimate (the rules data's fractions and why).
fn effects_json() -> Value {
    let r = rules();
    r.effects
        .keys()
        .map(|k| {
            let (lo, hi) = r.fractions(k);
            (k.clone(), json!({"low": lo, "high": hi, "why": r.why(k)}))
        })
        .collect::<Map<String, Value>>()
        .into()
}

// ---------------------------------------------------------------- commands

const SOURCES: [&str; 7] = ["finding", "question", "rfi", "validate", "coverage", "judged", "proposed"];

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let status = str_param(p, "status").unwrap_or("open");
    if !["open", "resolved", "all"].contains(&status) {
        return Err(bad(CMD, "`status` is open, resolved or all"));
    }
    let source = str_param(p, "source");
    if let Some(x) = source
        && !SOURCES.contains(&x)
    {
        return Err(bad(CMD, format!("`source` is one of {}", SOURCES.join(", "))));
    }
    let level = str_param(p, "level");
    let prescreen = bool_or(p, "prescreen", true);
    let (cursor, limit) = page(p);
    // The range needs every item; the list shows the ones asked for.
    let (items, _) = gather(s, &[], prescreen)?;
    let doc = std::sync::Arc::clone(&s.state()?.doc);
    let st = load(&doc);
    let pr = prices(s, &doc)?;
    let pricer = Pricer { d: &doc, p: &pr, twins: twins(&doc) };
    let all = rank(items, &pricer, &st);
    let estimate = range(&all, &pr);
    let mut by_source: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for x in &all {
        let e = by_source.entry(x.item.source).or_default();
        if x.status == "open" {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    let shown: Vec<(usize, &Ranked)> = all
        .iter()
        .filter(|x| match status {
            "open" => x.status == "open",
            "resolved" => x.status != "open",
            _ => true,
        })
        .filter(|x| source.is_none_or(|w| x.item.source == w))
        .filter(|x| level.is_none_or(|l| x.item.level.eq_ignore_ascii_case(l)))
        .enumerate()
        .collect();
    let total = shown.len();
    let mut page_items: Vec<Value> = Vec::new();
    for (n, x) in shown.iter().skip(cursor).take(limit) {
        let mut v = item_json(x, n + 1);
        if let Some(d) = st.decisions.get(&x.item.id) {
            v["resolution"] = json!({"action": d.action, "note": cut(&d.note, 200), "author": d.author, "seq": d.seq});
        }
        page_items.push(v);
    }
    let mut v = json!({
        "total": total,
        "open": all.iter().filter(|x| x.status == "open").count(),
        "resolved": all.iter().filter(|x| x.status != "open").count(),
        "by_source": by_source.iter().map(|(k, (o, r))| (k.to_string(), json!({"open": o, "resolved": r}))).collect::<Map<String, Value>>(),
        "estimate": estimate,
        "effects": effects_json(),
        "items": page_items,
        "how": "sorted by the most each item can move the estimate (its price times its kind's low and high fractions); items whose impact is unknown follow, the most severe first. review.impact.open {id, what: sheet} opens an item's source on its sheet (its `open.sheet`), {id, what: element} its elements (`open.element`); review.impact.resolve {id, action: accept|change|rfi_sent} settles it",
    });
    if !prescreen && let Some(o) = v.as_object_mut() {
        o.insert("note".into(), json!("Validate's rows left out (prescreen: false): the range counts the other sources only"));
    }
    Ok(paged(v, "items", cursor, total))
}

/// A page that fits the answer: items dropped from its end until it does, the next cursor after
/// the last one shown.
fn paged(mut v: Value, list: &str, cursor: usize, total: usize) -> Value {
    loop {
        let shown = v[list].as_array().map_or(0, Vec::len);
        if let Some(o) = v.as_object_mut() {
            if cursor + shown < total {
                o.insert("next_cursor".into(), json!(format!("c{}", cursor + shown)));
            } else {
                o.remove("next_cursor");
            }
        }
        if shown <= 1 || v.to_string().len() <= ANSWER_MAX {
            break;
        }
        if let Some(a) = v.get_mut(list).and_then(Value::as_array_mut) {
            a.pop();
        }
    }
    capped(v, list)
}

/// One item by id, gathered from its own source only.
fn find_item(s: &mut Session, cmd: &str, p: &Value) -> Result<(Item, Places)> {
    let id = str_param(p, "id")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad(cmd, "`id` is required: an item id from review.impact (RI-…)"))?;
    let source = source_of_id(id).ok_or_else(|| bad(cmd, format!("`{}` is no review.impact id (they start RI-)", cut(id, 40))))?;
    let (items, places) = gather(s, &[source], true)?;
    let item = items
        .into_iter()
        .find(|i| i.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| bad(cmd, format!("no review item `{}` now — call review.impact (a rebuild may have settled it)", cut(id, 40))))?;
    Ok((item, places))
}

fn run_resolve(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").map(str::trim).unwrap_or_default().to_string();
    if !["accept", "change", "rfi_sent", "reopen"].contains(&action.as_str()) {
        return Err(bad(RESOLVE, "`action` is accept, change, rfi_sent or reopen"));
    }
    let who = author(s, p, RESOLVE)?;
    let note = cut(str_param(p, "note").unwrap_or_default(), NOTE_MAX);
    let (item, _) = find_item(s, RESOLVE, p)?;
    let doc = std::sync::Arc::clone(&s.state()?.doc);
    let mut st = load(&doc);
    let pr = prices(s, &doc)?;
    let pricer = Pricer { d: &doc, p: &pr, twins: twins(&doc) };
    let ranked = rank(vec![item], &pricer, &st).into_iter().next().ok_or_else(|| bad(RESOLVE, "the item could not be priced"))?;
    let before = ranked.status.clone();
    let after = if action == "reopen" { "open".to_string() } else { action.clone() };
    if before == after {
        return Ok(json!({"id": ranked.item.id, "action": action, "status": before, "changed": false}));
    }
    if action == "rfi_sent" && ranked.item.rfi.is_none() && note.is_empty() {
        return Err(bad(RESOLVE, "rfi_sent on an item that isn't an RFI needs a `note` naming the RFI sent"));
    }
    st.seq += 1;
    let (dollars, low, high) =
        if ranked.impact.known { (Some(r2(ranked.impact.unit.dollars)), Some(r2(ranked.low)), Some(r2(ranked.high))) } else { (None, None, None) };
    st.trail.push(TrailEntry {
        seq: st.seq,
        id: ranked.item.id.clone(),
        action: action.clone(),
        before: before.clone(),
        note: note.clone(),
        author: who.clone(),
        title: ranked.item.title.clone(),
        source: ranked.item.source.to_string(),
        dollars,
        low,
        high,
    });
    if action == "reopen" {
        st.decisions.remove(&ranked.item.id);
    } else {
        st.decisions.insert(
            ranked.item.id.clone(),
            Decision {
                action: action.clone(),
                note: note.clone(),
                author: who,
                seq: st.seq,
                title: ranked.item.title.clone(),
                source: ranked.item.source.to_string(),
            },
        );
    }
    // An RFI marked sent is sent on the RFI list too.
    let mut rfi = Value::Null;
    if let Some(id) = &ranked.item.rfi
        && action == "rfi_sent"
        && ranked.item.source_status.is_none()
    {
        rfi = s.execute("rfi.set", &json!({"id": id, "status": "sent"})).map(|_| json!({"id": id, "status": "sent"})).unwrap_or(Value::Null);
    }
    save(s, &mut st)?;
    let r = rules();
    let moves = |x: &str| x == "open" || r.in_range(x);
    let mut v = json!({
        "id": ranked.item.id,
        "action": action,
        "status": after,
        "before": before,
        "title": ranked.item.title,
        "impact": if ranked.impact.known { json!({"dollars": dollars, "labor_hours": r2(ranked.impact.unit.hours)}) } else { json!("unknown") },
        "range": {"low": low, "high": high},
        "estimate_moves": moves(&after),
        "why": r.actions.get(&action).map_or("reopened: it moves the estimate again", |a| a.why.as_str()),
        "changed": true,
        "seq": st.seq,
        "next": "review.impact {} lists the items re-ranked",
    });
    if !rfi.is_null() {
        v["rfi"] = rfi;
    }
    Ok(v)
}

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let what = str_param(p, "what").unwrap_or("sheet");
    if !["sheet", "element"].contains(&what) {
        return Err(bad(OPEN, "`what` is sheet or element"));
    }
    let member = p.get("member").and_then(Value::as_u64).map(|m| m.min(1_000_000) as usize);
    let (item, places) = find_item(s, OPEN, p)?;
    if what == "sheet" {
        if let Some(sh) = &item.sheet {
            let half = WINDOW_FT * places.frame.upf();
            let (a, b) = ([sh.at[0] - half, sh.at[1] - half], [sh.at[0] + half, sh.at[1] + half]);
            s.execute("zoom", &json!({"mode": "window", "p1": a, "p2": b}))?;
            return Ok(json!({"id": item.id, "what": what, "sheet": sh.sheet, "at": sh.at, "window": [a[0], a[1], b[0], b[1]]}));
        }
        if let Some(id) = &item.rfi {
            let z = s.execute("rfi.zoom", &json!({"id": id}))?;
            return Ok(json!({"id": item.id, "what": what, "rfi": id, "window": z["window"]}));
        }
        return Err(bad(OPEN, format!("{} has no place on a sheet — open its element (what: element)", item.id)));
    }
    // The elements in the drawing: handles as they are, tag ids by the built element carrying them.
    let doc = std::sync::Arc::clone(&s.state()?.doc);
    let wanted: Vec<&String> = match member {
        Some(m) => item.handles.get(m).into_iter().collect(),
        None => item.handles.iter().collect(),
    };
    let mut handles: Vec<String> = Vec::new();
    let mut tags: HashSet<String> = HashSet::new();
    for h in &wanted {
        match Handle::parse_hex(h).filter(|x| doc.model.get(*x).is_some()) {
            Some(x) => handles.push(x.hex()),
            None => {
                tags.insert(h.trim().to_ascii_uppercase());
            }
        }
    }
    if !tags.is_empty() {
        for e in doc.model.iter().take(MAX_ENTITIES) {
            if handles.len() >= MAX_HANDLES {
                break;
            }
            if super::wallgen::made_as(e).is_none() {
                continue;
            }
            let (specific, shared) = keys_of(e);
            if specific.iter().skip(1).chain(shared.iter()).any(|k| tags.contains(k)) {
                handles.push(e.handle.hex());
            }
        }
    }
    if handles.is_empty() {
        return Err(bad(OPEN, format!("{} names no element in the model — open its sheet (what: sheet)", item.id)));
    }
    s.execute("zoom", &json!({"mode": "object", "handles": handles}))?;
    s.execute("select", &json!({"handles": handles}))?;
    Ok(json!({"id": item.id, "what": what, "handles": handles.iter().take(SHOWN_HANDLES).collect::<Vec<_>>(), "count": handles.len()}))
}

fn run_trail(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(s.doc()?);
    let id = str_param(p, "id").map(str::trim).filter(|x| !x.is_empty());
    let (cursor, limit) = page(p);
    let all: Vec<&TrailEntry> = st.trail.iter().rev().filter(|e| id.is_none_or(|i| e.id.eq_ignore_ascii_case(i))).collect();
    let entries: Vec<Value> = all.iter().skip(cursor).take(limit).map(|e| serde_json::to_value(e).unwrap_or(Value::Null)).collect();
    Ok(paged(json!({"total": all.len(), "entries": entries}), "entries", cursor, all.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_data_reads_and_every_effect_is_known() {
        let r = rules();
        assert!(!r.effects.is_empty() && r.rate() > 0.0);
        for e in r.verdicts.values().chain(r.rules.values().flat_map(|m| m.values())).chain(r.rfi_kinds.values()).chain([
            &r.finding,
            &r.question,
            &r.proposed,
        ]) {
            assert!(r.effects.contains_key(e), "effect `{e}` has no fractions");
            assert!(!r.why(e).is_empty(), "effect `{e}` says no why");
        }
        for a in ["accept", "change", "rfi_sent"] {
            assert!(r.actions.contains_key(a), "{a}");
        }
        assert_eq!(r.fractions("missing"), (0.0, 1.0));
        assert_eq!(r.fractions("nothing"), (0.0, 0.0));
        assert_eq!(r.of_verdict("G-COVERAGE", "fail"), "missing");
        assert_eq!(r.of_verdict("G-HOST", "fail"), "wrong");
        assert_eq!(r.of_rfi("anything"), "gap");
    }

    fn prices_of(lines: Vec<LineP>, per_key: &[(&str, f64)]) -> Prices {
        Prices {
            rate: 75.0,
            rate_from: String::new(),
            per_key: per_key.iter().map(|(k, d)| (k.to_string(), Unit { dollars: *d, hours: d / 100.0, ..Default::default() })).collect(),
            unpriced: HashSet::new(),
            lines,
            base: Unit::default(),
            priced_lines: 0,
            unpriced_lines: 0,
            st: Map::new(),
        }
    }

    fn line(category: &str, item: &str, size: &str, unit: &str, per: f64) -> LineP {
        LineP {
            category: category.into(),
            item: item.into(),
            size: size.into(),
            unit: unit.into(),
            net: 10.0,
            per: Unit { dollars: per, hours: per / 50.0, lines: vec![item.into()], ..Default::default() },
        }
    }

    #[test]
    fn coverage_fails_are_priced_short_by_their_marks_and_over_by_their_surplus() {
        let d = Drawing::default();
        let sp = BuildingSpec::default();
        let places = Places::of(&d, &sp, &json!({}));
        let rows = vec![
            json!({"id": "PS-1", "rule": "G-COVERAGE", "verdict": "fail", "kind": "motor_operated_damper", "issue": "MOTOR OPERATED DAMPER: 4 marks on X-2, 0 in the model",
                   "level": "L1", "at": [1.0, 2.0], "severity": "high", "measured": {"name": "motor operated damper", "codes": ["QMD"], "plan_marks": 4, "model": 0}}),
            json!({"id": "PS-2", "rule": "G-COVERAGE", "verdict": "fail", "kind": "volume_damper", "issue": "VOLUME DAMPER: 2 marks, 3 in the model",
                   "level": "L1", "severity": "medium", "measured": {"name": "volume damper", "plan_marks": 2, "model": 3, "model_elements": ["1A", "1B", "1C"]}}),
            json!({"id": "PS-3", "rule": "G-COVERAGE", "verdict": "fail", "kind": "smoke_hatch", "issue": "SMOKE HATCH: 1 mark, 0 in the model",
                   "level": "L1", "severity": "high", "measured": {"name": "smoke hatch", "plan_marks": 1, "model": 0}}),
            json!({"id": "PS-4", "rule": "G-HOST", "verdict": "pass", "handle": "1A"}),
            json!({"id": "PS-5", "rule": "G-PRESENT", "verdict": "missing", "issue": "Room 7 holds 0 of the 2 thermostat its type expects", "level": "L1",
                   "measured": {"item": "thermostat=2+", "count": 0}}),
            json!({"id": "PS-6", "rule": "G-PRESENT", "verdict": "missing", "issue": "Room 8 holds 0 of the 2 thermostat its type expects", "level": "L1",
                   "measured": {"item": "thermostat=2+", "count": 1}}),
        ];
        let mut items = Vec::new();
        validate_items(&rows, &places, &mut items);
        assert_eq!(
            items.iter().map(|i| i.source).collect::<Vec<_>>(),
            vec!["coverage", "coverage", "coverage", "validate"],
            "the pass is left out, the two rooms' rows are one"
        );
        assert!(items.iter().take(3).all(|i| i.effect == "missing"));
        let p = prices_of(
            vec![
                line("dampers", "damper actuator QMD", "", "each", 120.0),
                line("devices", "thermostat", "", "each", 80.0),
                line("conduit", "EMT conduit", "3/4\"", "ft", 2.0),
            ],
            &[("1A", 10.0), ("1B", 10.0), ("1C", 10.0)],
        );
        let pricer = Pricer { d: &d, p: &p, twins: HashMap::new() };
        let short = pricer.impact(&items[0]);
        assert!(short.known && (short.unit.dollars - 480.0).abs() < 1e-6, "{short:?}");
        assert!(short.parts.is_empty() && (short.loose.0 - 480.0).abs() < 1e-6);
        let over = pricer.impact(&items[1]);
        assert!(over.known && (over.unit.dollars - 10.0).abs() < 1e-6 && over.parts.len() == 3, "a third of three elements: {over:?}");
        let none = pricer.impact(&items[2]);
        assert!(!none.known && none.basis.join(" ").contains("no takeoff line or price"), "{none:?}");
        // Two rooms short of 2 and 1 thermostats: 3 priced like the thermostat line.
        assert_eq!(items[3].count, 2);
        let t = pricer.impact(&items[3]);
        assert!((t.unit.dollars - 240.0).abs() < 1e-6, "{t:?}");
        // A proposed run by the foot like its size's line.
        let mut run = Item::new("proposed", "run");
        run.run = Some(("3/4\"".into(), 25.0));
        assert!((pricer.impact(&run).unit.dollars - 50.0).abs() < 1e-6);
    }

    #[test]
    fn the_range_counts_an_element_once_at_its_widest_swing() {
        let d = Drawing::default();
        let p = prices_of(Vec::new(), &[("1A", 100.0), ("1B", 40.0)]);
        let pricer = Pricer { d: &d, p: &p, twins: HashMap::new() };
        let mk = |key: &str, effect: &str, hs: &[&str]| {
            let mut i = Item::new("validate", key);
            i.effect = effect.into();
            for h in hs {
                i.add_handle(h);
            }
            i
        };
        // Two items on 1A (wrong ±50, unsure ±25) and one extra on 1B (−40..0).
        let items = vec![mk("a", "wrong", &["1A"]), mk("b", "unsure", &["1A"]), mk("c", "extra", &["1B"]), mk("d", "finding", &[])];
        let st = State::default();
        let ranked = rank(items, &pricer, &st);
        assert_eq!(ranked.iter().map(|x| x.item.effect.as_str()).collect::<Vec<_>>(), vec!["wrong", "extra", "unsure", "finding"]);
        let r = range(&ranked, &p);
        assert_eq!((r["low"].as_f64(), r["high"].as_f64()), (Some(0.0), Some(50.0)), "base 0: low −90 floors at 0, high +50: {r}");
        assert_eq!((r["unresolved"].as_u64(), r["unknown_impact"].as_u64()), (Some(4), Some(1)));
        // Accepted: out of the range; sent: still in it.
        let mut st = State::default();
        st.decisions.insert(mk("a", "wrong", &[]).id, Decision { action: "accept".into(), ..Default::default() });
        st.decisions.insert(mk("b", "unsure", &[]).id, Decision { action: "rfi_sent".into(), ..Default::default() });
        let items = vec![mk("a", "wrong", &["1A"]), mk("b", "unsure", &["1A"]), mk("c", "extra", &["1B"])];
        let r = range(&rank(items, &pricer, &st), &prices_of(Vec::new(), &[]));
        assert_eq!((r["unresolved"].as_u64(), r["high"].as_f64()), (Some(2), Some(25.0)), "the sent one still swings 1A: {r}");
    }

    #[test]
    fn rows_differing_only_in_numbers_word_alike() {
        assert_eq!(wording("off the face by 7.25 in at 3 ft"), wording("off the face by 12 in at 40 ft"));
        assert_ne!(wording("no maker for this switch"), wording("no maker for this receptacle"));
        assert_eq!(source_of_id("RI-V1234abcd"), Some("validate"));
        assert_eq!(source_of_id("RV-J1"), None);
        assert_eq!(source_of_id("RI-"), None);
    }
}
