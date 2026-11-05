//! The rules an agent follows to check the model against the prints (Q6114): what must be true
//! of each kind of element, which print views show it, the steps to compare them, how close is
//! close enough and how much a miss matters. The deterministic checks of [`super::validate`]
//! (`bim.validate`) pre-screen the measurable part of a rule (`prescreen`); the rest is judged
//! by a person or a connected agent looking at the prints.
//!
//! The rules are data compiled into the engine (`validate_rules.json`). A project changes a
//! rule's tolerances, severity or whether it is used at all through the project rules
//! (`project.rules.set`, or [`CMD_SET`] which writes them for you):
//!
//! - domain `tolerance`, key `validate.<RULE>.tolerance.<name>`, value a number;
//! - domain `custom`, key `validate.<RULE>.severity`, value `high`, `medium` or `low`;
//! - domain `custom`, key `validate.<RULE>.enabled`, value `true` or `false`.
//!
//! Commands:
//! - `validate.rules {trade?, kind?, id?, detail?, cursor?, limit?}` — the rules, paged, with the
//!   project's changes applied; with `kind`, the rules an element of that kind gets;
//! - `validate.rules.set {id, tolerance?, severity?, enabled?, reset?, reason}` — change a rule
//!   for this project.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_doc::projrules::{ProjectRules, RuleDomain, RuleValue, SourceTier};
use cadaddy_walk3d::validate::Check;
use cadaddy_walk3d::validate_kinds;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::agentready::ANSWER_MAX;
use super::*;
use crate::{Result, Session};

pub const CMD_RULES: &str = "validate.rules";
pub const CMD_SET: &str = "validate.rules.set";

/// The built-in rules, as data.
const RULES_JSON: &str = include_str!("validate_rules.json");
/// The trades a rule belongs to.
pub const TRADES: [&str; 6] = ["architecture", "electrical", "mechanical", "plumbing", "fire", "general"];
/// The kinds of print document a rule looks at.
pub const DOCUMENTS: [&str; 12] = [
    "plan",
    "enlarged_plan",
    "rcp",
    "interior_elevation",
    "section",
    "schedule",
    "legend",
    "general_notes",
    "spec",
    "submittal",
    "partition_legend",
    "riser_diagram",
];
/// The tolerances a rule may have (inches unless the name says feet).
pub const TOLERANCES: [&str; 7] = ["plan_in", "height_in", "route_ft", "size_in", "depth_in", "clear_width_in", "clear_depth_in"];
pub const SEVERITIES: [&str; 3] = ["high", "medium", "low"];
/// Largest tolerance a project may set.
const MAX_TOLERANCE: f64 = 1_000.0;
/// The project-rule key prefix of a rule override.
const KEY_PREFIX: &str = "validate.";
const PAGE_DEFAULT: usize = 10;
const PAGE_MAX: usize = 50;
/// Longest text read from a parameter.
const MAX_TEXT: usize = 200;

/// What a rule applies to: element kinds (`*` = every element), BIM categories (the IFC
/// classes elements are written as) and type mark patterns (`EF-*`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliesTo {
    pub kinds: Vec<String>,
    pub categories: Vec<String>,
    pub marks: Vec<String>,
}

/// One generic passing and one failing case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Examples {
    pub pass: String,
    pub fail: String,
}

/// Where a code requirement holds (a rule that checks one, e.g. E-RECEPT-TR): the code text it
/// rests on and the rooms it covers, as data. A project adds room names with
/// `validate.<RULE>.rooms` (domain `custom`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeScope {
    /// The code section (`NEC 406.12`).
    pub cite: String,
    /// What it requires, in short.
    pub says: String,
    /// The room names, unit types or room types it covers (`guest room`, `dwelling unit`).
    pub rooms: Vec<String>,
    /// Every room with a typical unit type (a guest room or dwelling unit of the job) is covered.
    #[serde(default)]
    pub typical_units: bool,
    /// The device ratings it covers, at most (a receptacle known to be rated above is not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_amps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_volts: Option<f64>,
}

/// Room names a code scope lists (built in and a project's), at most; longest name.
pub const MAX_CODE_ROOMS: usize = 100;
const MAX_ROOM_NAME: usize = 60;

/// A validation rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub trade: String,
    pub applies_to: AppliesTo,
    /// What must be true, in one sentence.
    pub check: String,
    /// The steps an agent follows: which print views to look at and what to compare.
    pub how: Vec<String>,
    pub documents: Vec<String>,
    pub tolerance: BTreeMap<String, f64>,
    pub severity: String,
    /// The `bim.validate` check that covers the measurable part, if any.
    pub prescreen: Option<String>,
    pub examples: Examples,
    /// The code requirement the rule checks and where it holds, when it checks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeScope>,
}

impl Rule {
    /// Does an element of `kind` get this rule? By the kind families of
    /// [`validate_kinds::kind_matches`] (one table for every rule): a `gfci_receptacle` gets the
    /// `receptacle` rules and a `return_grille` the `terminal` ones, a `light_fixture` not the
    /// plumbing `fixture` ones.
    pub fn applies_to_kind(&self, kind: &str) -> bool {
        self.applies_to.kinds.iter().any(|r| validate_kinds::kind_matches(r, kind))
    }

    /// Does an element get this rule: by its kind, its BIM category (the IFC class it is
    /// written as) or its mark (`EF-*`)?
    pub fn applies_to_element(&self, kind: &str, category: Option<&str>, mark: Option<&str>) -> bool {
        self.applies_to_kind(kind)
            || category.is_some_and(|c| self.applies_to.categories.iter().any(|x| x.eq_ignore_ascii_case(c.trim())))
            || mark.is_some_and(|m| self.applies_to.marks.iter().any(|p| mark_matches(p, m)))
    }

    /// Does this rule name kinds of element (not every element, `*`)?
    pub fn names_kinds(&self) -> bool {
        !self.applies_to.kinds.iter().any(|k| k == "*")
    }

    /// What is wrong with the rule, if anything.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let id = &self.id;
        let valid_id = !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-');
        if !valid_id {
            out.push(format!("rule id `{id}` must be upper-case letters, digits and dashes"));
        }
        if !TRADES.contains(&self.trade.as_str()) {
            out.push(format!("{id}: trade `{}` is not one of {}", self.trade, TRADES.join(", ")));
        }
        if self.applies_to.kinds.is_empty() && self.applies_to.categories.is_empty() && self.applies_to.marks.is_empty() {
            out.push(format!("{id}: applies_to names nothing"));
        }
        for k in &self.applies_to.kinds {
            if k != "*" && !validate_kinds::is_rule_kind(k) {
                out.push(format!("{id}: kind `{k}` is not a kind the model holds or a kind family"));
            }
        }
        let cats = categories();
        for c in &self.applies_to.categories {
            if !cats.contains(c) {
                out.push(format!("{id}: category `{c}` is not one the model's elements are written as"));
            }
        }
        for m in &self.applies_to.marks {
            if m.trim().is_empty() {
                out.push(format!("{id}: an empty type mark pattern"));
            }
        }
        if self.check.trim().is_empty() {
            out.push(format!("{id}: no check"));
        }
        if self.how.is_empty() || self.how.iter().any(|s| s.trim().is_empty()) {
            out.push(format!("{id}: `how` needs its steps"));
        }
        if self.documents.is_empty() {
            out.push(format!("{id}: no documents"));
        }
        for d in &self.documents {
            if !DOCUMENTS.contains(&d.as_str()) {
                out.push(format!("{id}: document `{d}` is not one of {}", DOCUMENTS.join(", ")));
            }
        }
        for (k, v) in &self.tolerance {
            if let Err(e) = tolerance_value(k, *v) {
                out.push(format!("{id}: {e}"));
            }
        }
        if !SEVERITIES.contains(&self.severity.as_str()) {
            out.push(format!("{id}: severity `{}` is not high, medium or low", self.severity));
        }
        if let Some(p) = &self.prescreen
            && Check::parse(p).is_none()
        {
            out.push(format!("{id}: prescreen `{p}` is not a bim.validate check"));
        }
        if self.examples.pass.trim().is_empty() || self.examples.fail.trim().is_empty() {
            out.push(format!("{id}: needs a pass and a fail example"));
        }
        if let Some(c) = &self.code {
            if c.cite.trim().is_empty() || c.says.trim().is_empty() {
                out.push(format!("{id}: its code needs the section it cites and what it says"));
            }
            if c.rooms.is_empty() && !c.typical_units {
                out.push(format!("{id}: its code covers no rooms"));
            }
            if c.rooms.len() > MAX_CODE_ROOMS || c.rooms.iter().any(|r| r.trim().is_empty() || r.chars().count() > MAX_ROOM_NAME) {
                out.push(format!("{id}: its code lists at most {MAX_CODE_ROOMS} rooms, each 1 to {MAX_ROOM_NAME} characters"));
            }
            if [c.max_amps, c.max_volts].iter().flatten().any(|x| !x.is_finite() || *x <= 0.0) {
                out.push(format!("{id}: its code's ratings must be positive numbers"));
            }
        }
        out
    }
}

/// Does mark `m` fit pattern `p` (any case; `*` stands for any characters)?
pub fn mark_matches(p: &str, m: &str) -> bool {
    let (p, m) = (p.trim().to_ascii_uppercase(), m.trim().to_ascii_uppercase());
    if m.is_empty() || p.is_empty() {
        return false;
    }
    let parts: Vec<&str> = p.split('*').collect();
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else { return false };
    if parts.len() == 1 {
        return p == m;
    }
    if !m.starts_with(first) || !m.get(first.len()..).is_some_and(|rest| rest.ends_with(last)) || m.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = m.get(first.len()..m.len().saturating_sub(last.len())).unwrap_or_default();
    for mid in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        match rest.find(mid) {
            Some(i) => rest = rest.get(i + mid.len()..).unwrap_or_default(),
            None => return false,
        }
    }
    true
}

fn categories() -> &'static BTreeSet<String> {
    static CATS: OnceLock<BTreeSet<String>> = OnceLock::new();
    CATS.get_or_init(validate_kinds::model_categories)
}

fn tolerance_value(name: &str, v: f64) -> std::result::Result<f64, String> {
    if !TOLERANCES.contains(&name) {
        return Err(format!("tolerance `{name}` is not one of {}", TOLERANCES.join(", ")));
    }
    if !v.is_finite() || !(0.0..=MAX_TOLERANCE).contains(&v) {
        return Err(format!("tolerance `{name}` must be a number from 0 to {MAX_TOLERANCE}"));
    }
    Ok(v)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[allow(dead_code)]
    version: u32,
    rules: Vec<Rule>,
}

/// Parse and check a rule set.
pub fn parse_rules(text: &str) -> std::result::Result<Vec<Rule>, String> {
    let f: RuleFile = serde_json::from_str(text).map_err(|e| format!("the validation rules do not read: {e}"))?;
    let mut ids = BTreeSet::new();
    let mut problems = Vec::new();
    for r in &f.rules {
        if !ids.insert(r.id.clone()) {
            problems.push(format!("rule id `{}` is used twice", r.id));
        }
        problems.extend(r.problems());
    }
    if problems.is_empty() { Ok(f.rules) } else { Err(problems.join("; ")) }
}

/// The built-in rules.
pub fn builtin() -> std::result::Result<&'static [Rule], String> {
    static RULES: OnceLock<std::result::Result<Vec<Rule>, String>> = OnceLock::new();
    match RULES.get_or_init(|| parse_rules(RULES_JSON)) {
        Ok(r) => Ok(r.as_slice()),
        Err(e) => Err(e.clone()),
    }
}

// ---------------------------------------------------------------- project overrides

/// What a project override changes.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Tolerance(String, f64),
    Severity(String),
    Enabled(bool),
    /// Room names the rule's code scope covers too.
    Rooms(Vec<String>),
}

/// Room names as a project writes them (`dorm suite; sleeping room`): `;` or `,` between.
pub fn parse_rooms(value: &str) -> std::result::Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in value.split([';', ',', '\n']).map(str::trim).filter(|r| !r.is_empty()) {
        if r.chars().count() > MAX_ROOM_NAME || r.chars().any(char::is_control) {
            return Err(format!("a room name is at most {MAX_ROOM_NAME} printable characters"));
        }
        if out.len() >= MAX_CODE_ROOMS {
            return Err(format!("at most {MAX_CODE_ROOMS} room names"));
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(r)) {
            out.push(r.to_string());
        }
    }
    if out.is_empty() {
        return Err("name at least one room (e.g. `dorm suite; sleeping room`)".into());
    }
    Ok(out)
}

/// A project override, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub rule: String,
    pub change: Change,
}

/// Is `key` (a project rule key) a validation rule override?
pub fn is_override_key(key: &str) -> bool {
    key.trim().to_ascii_lowercase().starts_with(KEY_PREFIX) && !super::validate_kind::is_kind_key(key) && !super::validate_height::is_height_key(key)
}

/// Read a project rule as a validation rule override: the rule must exist, the field be one a
/// project may change, the value fit it and the domain be the one that field lives in.
pub fn check_override(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<Override, String> {
    let rules = builtin()?;
    let rest =
        key.trim().get(KEY_PREFIX.len()..).filter(|_| is_override_key(key)).ok_or_else(|| format!("`{key}` does not start with `validate.`"))?;
    let mut parts = rest.split('.');
    let id = parts.next().unwrap_or_default().trim().to_ascii_uppercase();
    let rule = rules.iter().find(|r| r.id == id).ok_or_else(|| format!("no validation rule `{id}` (see validate.rules)"))?;
    let field: Vec<&str> = parts.collect();
    let v = value.trim();
    let change = match field.as_slice() {
        ["tolerance", name] => {
            if domain != RuleDomain::Tolerance {
                return Err(format!("`{key}` is a tolerance: set it in domain `tolerance`"));
            }
            let n: f64 = v.trim_end_matches(['"', '\'']).trim().parse().map_err(|_| format!("`{key}` must be a number, not `{v}`"))?;
            Change::Tolerance(name.to_string(), tolerance_value(name, n)?)
        }
        ["severity"] | ["enabled"] | ["rooms"] if domain != RuleDomain::Custom => return Err(format!("`{key}`: set it in domain `custom`")),
        ["rooms"] if rule.code.is_none() => return Err(format!("`{key}`: rule {id} checks no code requirement, so it has no rooms to add to")),
        ["rooms"] => Change::Rooms(parse_rooms(v).map_err(|e| format!("`{key}`: {e}"))?),
        ["severity"] => {
            let s = v.to_ascii_lowercase();
            if !SEVERITIES.contains(&s.as_str()) {
                return Err(format!("`{key}` must be high, medium or low, not `{v}`"));
            }
            Change::Severity(s)
        }
        ["enabled"] => Change::Enabled(match v.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" => false,
            _ => return Err(format!("`{key}` must be true or false, not `{v}`")),
        }),
        _ => {
            return Err(format!(
                "`{key}`: a project may change `validate.{id}.tolerance.<name>`, `validate.{id}.severity`, `validate.{id}.enabled` or (a rule with a code requirement) `validate.{id}.rooms`"
            ));
        }
    };
    Ok(Override { rule: rule.id.clone(), change })
}

/// A rule as this project uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub rule: Rule,
    pub enabled: bool,
    /// The fields the project changed: (field, built-in value, project value, project rule key).
    pub changed: Vec<(String, Value, Value, String)>,
}

impl Effective {
    pub fn source(&self) -> &'static str {
        if self.changed.is_empty() { "built-in" } else { "project" }
    }

    pub fn to_json(&self, detail: bool) -> Value {
        let r = &self.rule;
        let mut v = json!({
            "id": r.id,
            "trade": r.trade,
            "applies_to": r.applies_to,
            "check": r.check,
            "documents": r.documents,
            "tolerance": r.tolerance,
            "severity": r.severity,
            "prescreen": r.prescreen,
            "code": r.code,
            "enabled": self.enabled,
            "source": self.source(),
        });
        if let Some(o) = v.as_object_mut() {
            if detail {
                o.insert("how".into(), json!(r.how));
                o.insert("examples".into(), json!(r.examples));
            }
            if !self.changed.is_empty() {
                let ch: Vec<Value> = self.changed.iter().map(|(f, b, p, k)| json!({"field": f, "built_in": b, "project": p, "key": k})).collect();
                o.insert("overrides".into(), Value::Array(ch));
            }
        }
        v
    }
}

/// A project rule that names a validation rule but was not used, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Rejected {
    pub key: String,
    pub value: String,
    pub why: String,
}

/// The rules with the project's overrides applied, and the overrides that were refused.
pub fn effective(pr: &ProjectRules) -> std::result::Result<(Vec<Effective>, Vec<Rejected>), String> {
    let mut out: Vec<Effective> = builtin()?.iter().map(|r| Effective { rule: r.clone(), enabled: true, changed: Vec::new() }).collect();
    let mut rejected = Vec::new();
    for e in pr.list(None) {
        if !matches!(e.domain, RuleDomain::Tolerance | RuleDomain::Custom) || !is_override_key(&e.key) {
            continue;
        }
        let key = format!("{}/{}", e.domain.as_str(), e.key);
        let value = e.active.value.clone();
        let o = match check_override(e.domain, &e.key, &value) {
            Ok(o) => o,
            Err(why) => {
                rejected.push(Rejected { key, value, why });
                continue;
            }
        };
        let Some(eff) = out.iter_mut().find(|x| x.rule.id == o.rule) else { continue };
        match o.change {
            Change::Tolerance(name, n) => {
                let was = eff.rule.tolerance.insert(name.clone(), n).map_or(Value::Null, |b| json!(b));
                eff.changed.push((format!("tolerance.{name}"), was, json!(n), key));
            }
            Change::Severity(s) => {
                let was = std::mem::replace(&mut eff.rule.severity, s.clone());
                eff.changed.push(("severity".into(), json!(was), json!(s), key));
            }
            Change::Enabled(b) => {
                eff.enabled = b;
                eff.changed.push(("enabled".into(), json!(true), json!(b), key));
            }
            Change::Rooms(rooms) => {
                if let Some(code) = eff.rule.code.as_mut() {
                    let was = json!(code.rooms);
                    for r in rooms {
                        if code.rooms.len() < MAX_CODE_ROOMS && !code.rooms.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
                            code.rooms.push(r);
                        }
                    }
                    let now = json!(code.rooms);
                    eff.changed.push(("code.rooms".into(), was, now, key));
                }
            }
        }
    }
    Ok((out, rejected))
}

// ---------------------------------------------------------------- commands

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_RULES, "Validation Rules", run_rules)
            .params(
                "{trade?: string (architecture|electrical|mechanical|plumbing|fire|general), kind?: string (an element kind: the rules an element of that kind gets, the general ones too), id?: string (one rule), detail?: bool (the steps and examples; default true), cursor?: integer, limit?: integer (default 10, at most 50)} → {total, rules: [{id, trade, applies_to: {kinds, categories, marks}, check, how, documents, tolerance, severity, prescreen, examples, enabled, source: built-in|project, overrides?: [{field, built_in, project, key}]}], next?, rejected: [{key, value, why}]}: the rules an agent follows to check the model against the prints, with this project's changes",
            )
            .noundo(),
        CommandSpec::new(CMD_SET, "Change a Validation Rule", run_set).params(
            "{id: string (a rule id from validate.rules), tolerance?: {name: number} (plan_in, height_in, route_ft, size_in, depth_in, clear_width_in, clear_depth_in), severity?: string (high|medium|low), enabled?: bool, rooms?: [string] (more room names a rule with a code requirement covers, e.g. E-RECEPT-TR), reset?: bool (drop this project's changes to the rule first), reason: string} → {rule, set: [key]}: change a rule for this project (kept in the project rules)",
        ),
    ]
}

fn text_param<'a>(cmd: &str, p: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().count() <= MAX_TEXT => Ok(Some(s.trim())),
        Some(Value::String(_)) => Err(bad(cmd, format!("`{key}` is longer than {MAX_TEXT} characters"))),
        Some(_) => Err(bad(cmd, format!("`{key}` must be a string"))),
    }
}

fn bool_param(cmd: &str, p: &Value, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(cmd, format!("`{key}` must be true or false"))),
    }
}

fn count_param(cmd: &str, p: &Value, key: &str) -> Result<Option<usize>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(|n| Some(n.min(1_000_000) as usize)).ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number, 0 or more"))),
    }
}

fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = CMD_RULES;
    let trade = text_param(cmd, p, "trade")?.map(str::to_ascii_lowercase);
    if let Some(t) = &trade
        && !TRADES.contains(&t.as_str())
    {
        return Err(bad(cmd, format!("`trade` must be one of {}", TRADES.join(", "))));
    }
    let kind = text_param(cmd, p, "kind")?.filter(|k| !k.is_empty());
    let id = text_param(cmd, p, "id")?.map(str::to_ascii_uppercase);
    let detail = bool_param(cmd, p, "detail")?.unwrap_or(true);
    let cursor = count_param(cmd, p, "cursor")?.unwrap_or(0);
    let limit = count_param(cmd, p, "limit")?.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);

    let (rules, rejected) = effective(&s.doc()?.project_rules).map_err(|e| bad(cmd, e))?;
    if let Some(id) = &id
        && !rules.iter().any(|r| &r.rule.id == id)
    {
        return Err(bad(cmd, format!("no validation rule `{id}` (validate.rules lists them)")));
    }
    let picked: Vec<&Effective> = rules
        .iter()
        .filter(|r| id.as_ref().is_none_or(|i| &r.rule.id == i))
        .filter(|r| trade.as_ref().is_none_or(|t| &r.rule.trade == t))
        .filter(|r| kind.is_none_or(|k| r.rule.applies_to_kind(k)))
        .collect();
    let total = picked.len();
    let mut page: Vec<Value> = picked.iter().skip(cursor).take(limit).map(|r| r.to_json(detail)).collect();
    let rejected: Vec<Value> = rejected.iter().take(50).map(|r| json!({"key": r.key, "value": r.value, "why": r.why})).collect();
    // Keep the answer within what an agent is handed at once: fewer rules on this page.
    loop {
        let shown = page.len();
        let next = cursor.saturating_add(shown);
        let mut v = json!({"total": total, "cursor": cursor, "rules": page, "rejected": rejected});
        if let Some(o) = v.as_object_mut() {
            if next < total {
                o.insert("next".into(), json!(next));
            }
            if let Some(k) = kind {
                o.insert("kind".into(), json!(k));
                o.insert("kind_known".into(), json!(validate_kinds::is_model_kind(k)));
            }
        }
        if v.to_string().len() <= ANSWER_MAX || page.len() <= 1 {
            return Ok(v);
        }
        page = match v.get_mut("rules").map(Value::take) {
            Some(Value::Array(mut a)) => {
                a.pop();
                a
            }
            _ => Vec::new(),
        };
    }
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = CMD_SET;
    let id = text_param(cmd, p, "id")?.map(str::to_ascii_uppercase).ok_or_else(|| bad(cmd, "`id` (a rule id) is required"))?;
    let reason = text_param(cmd, p, "reason")?
        .filter(|r| !r.is_empty())
        .ok_or_else(|| bad(cmd, "`reason` is required: say why the project changes this rule"))?;
    let builtin = builtin().map_err(|e| bad(cmd, e))?;
    if !builtin.iter().any(|r| r.id == id) {
        return Err(bad(cmd, format!("no validation rule `{id}` (validate.rules lists them)")));
    }
    // Everything to write, checked before anything changes.
    let mut writes: Vec<(RuleDomain, String, String)> = Vec::new();
    match p.get("tolerance") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (name, v) in m.iter().take(TOLERANCES.len() + 1) {
                let n = v.as_f64().ok_or_else(|| bad(cmd, format!("tolerance `{name}` must be a number")))?;
                writes.push((RuleDomain::Tolerance, format!("{KEY_PREFIX}{id}.tolerance.{name}"), format!("{n}")));
            }
        }
        Some(_) => return Err(bad(cmd, "`tolerance` must be an object of name: number")),
    }
    if let Some(sv) = text_param(cmd, p, "severity")? {
        writes.push((RuleDomain::Custom, format!("{KEY_PREFIX}{id}.severity"), sv.to_ascii_lowercase()));
    }
    if let Some(b) = bool_param(cmd, p, "enabled")? {
        writes.push((RuleDomain::Custom, format!("{KEY_PREFIX}{id}.enabled"), b.to_string()));
    }
    match p.get("rooms") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            let names: Vec<&str> = a
                .iter()
                .take(MAX_CODE_ROOMS + 1)
                .map(|r| r.as_str().map(str::trim).ok_or_else(|| bad(cmd, "`rooms` is a list of room names")))
                .collect::<Result<_>>()?;
            if names.iter().any(|r| r.contains([';', ','])) {
                return Err(bad(cmd, "a room name in `rooms` has no `;` or `,`"));
            }
            writes.push((RuleDomain::Custom, format!("{KEY_PREFIX}{id}.rooms"), names.join("; ")));
        }
        Some(_) => return Err(bad(cmd, "`rooms` is a list of room names")),
    }
    let reset = bool_param(cmd, p, "reset")?.unwrap_or(false);
    if writes.is_empty() && !reset {
        return Err(bad(cmd, "nothing to change: give `tolerance`, `severity`, `enabled`, `rooms` or `reset`"));
    }
    for (domain, key, value) in &writes {
        check_override(*domain, key, value).map_err(|e| bad(cmd, e))?;
    }
    let d = s.doc_mut()?;
    if reset {
        let prefix = format!("{}{}.", KEY_PREFIX, id.to_ascii_lowercase());
        let mine: Vec<(RuleDomain, String)> = d
            .project_rules
            .list(None)
            .into_iter()
            .filter(|e| e.key.trim().to_ascii_lowercase().starts_with(&prefix))
            .map(|e| (e.domain, e.key.clone()))
            .collect();
        for (domain, key) in mine {
            d.project_rules.remove(domain, &key);
        }
    }
    let mut set = Vec::new();
    for (domain, key, value) in writes {
        d.project_rules.set(domain, &key, RuleValue::new(value).with_source(CMD_SET).with_tier(SourceTier::ProjectRule), reason);
        set.push(format!("{}/{}", domain.as_str(), key));
    }
    let (rules, _) = effective(&d.project_rules).map_err(|e| bad(cmd, e))?;
    let rule = rules.iter().find(|r| r.rule.id == id).map(|r| r.to_json(false)).unwrap_or(Value::Null);
    Ok(json!({"rule": rule, "set": set, "reset": reset}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).ok();
        s
    }

    #[test]
    fn every_rule_parses_and_has_every_field() {
        let rules = builtin().unwrap();
        assert!(rules.len() >= 20, "{}", rules.len());
        // Each rule object in the data names every field (none left to a default).
        let raw: Value = serde_json::from_str(RULES_JSON).unwrap();
        let fields = ["id", "trade", "applies_to", "check", "how", "documents", "tolerance", "severity", "prescreen", "examples"];
        for r in raw["rules"].as_array().unwrap() {
            for f in fields {
                assert!(r.get(f).is_some(), "{} has no `{f}`", r["id"]);
            }
            for f in ["kinds", "categories", "marks"] {
                assert!(r["applies_to"].get(f).is_some(), "{} applies_to has no `{f}`", r["id"]);
            }
        }
        for r in rules {
            assert!(r.problems().is_empty(), "{:?}", r.problems());
        }
    }

    #[test]
    fn ids_are_unique_and_the_starting_rules_are_there() {
        let rules = builtin().unwrap();
        let ids: BTreeSet<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), rules.len());
        for want in [
            "G-PRESENT",
            "G-SOURCED",
            "G-PLACE",
            "G-TYPE",
            "G-ROOM",
            "G-HOST",
            "E-RECEPT-POS",
            "E-RECEPT-TYPE",
            "E-SWITCH",
            "E-PANEL-POS",
            "E-PANEL-MOUNT",
            "E-LUM",
            "E-FA",
            "E-CIRCUIT",
            "M-EF",
            "M-DUCT-ROUTE",
            "M-DUCT-SIZE",
            "M-DUCT-HEIGHT",
            "M-TERMINAL",
            "M-EQUIP",
            "P-FIXTURE",
            "P-PIPE",
            "F-HEAD",
        ] {
            assert!(ids.contains(want), "{want}");
        }
        // A duplicate id is refused.
        let mut raw: Value = serde_json::from_str(RULES_JSON).unwrap();
        let first = raw["rules"][0].clone();
        raw["rules"].as_array_mut().unwrap().push(first);
        assert!(parse_rules(&raw.to_string()).unwrap_err().contains("used twice"));
    }

    #[test]
    fn applies_to_kinds_are_kinds_the_model_holds() {
        for r in builtin().unwrap() {
            for k in &r.applies_to.kinds {
                assert!(k == "*" || validate_kinds::is_rule_kind(k), "{}: {k}", r.id);
            }
            for c in &r.applies_to.categories {
                assert!(validate_kinds::model_categories().contains(c), "{}: {c}", r.id);
            }
            if let Some(p) = &r.prescreen {
                assert!(Check::parse(p).is_some(), "{}: {p}", r.id);
            }
        }
        // A rule naming a kind the model never makes is refused.
        let mut raw: Value = serde_json::from_str(RULES_JSON).unwrap();
        raw["rules"][0]["applies_to"]["kinds"] = json!(["unicorn"]);
        raw["rules"][1]["prescreen"] = json!("vibes");
        raw["rules"][2]["documents"] = json!(["napkin"]);
        let e = parse_rules(&raw.to_string()).unwrap_err();
        assert!(e.contains("`unicorn` is not a kind") && e.contains("`vibes`") && e.contains("`napkin`"), "{e}");
        // A missing field does not read.
        let mut raw: Value = serde_json::from_str(RULES_JSON).unwrap();
        raw["rules"][0].as_object_mut().unwrap().remove("examples");
        assert!(parse_rules(&raw.to_string()).is_err());
    }

    #[test]
    fn an_element_kind_gets_its_rules_and_the_general_ones() {
        let rules = builtin().unwrap();
        let got = |k: &str| -> BTreeSet<&str> { rules.iter().filter(|r| r.applies_to_kind(k)).map(|r| r.id.as_str()).collect() };
        let fan = got("exhaust_fan");
        assert!(fan.contains("M-EF") && fan.contains("G-PRESENT") && !fan.contains("E-RECEPT-POS"), "{fan:?}");
        let gfci = got("GFCI Receptacle");
        assert!(gfci.contains("E-RECEPT-TYPE") && gfci.contains("G-HOST"), "{gfci:?}");
        assert!(!got("pipe").contains("M-DUCT-SIZE"));
        assert!(got("duct").contains("M-DUCT-SIZE"));
    }

    /// The kinds the job builds get their trade's rules through the family table (Q6131).
    #[test]
    fn the_jobs_kinds_get_rules_by_family() {
        let rules = builtin().unwrap();
        // G-PRODUCT (Q6137) checks every product kind's record: asserted on its own below, left
        // out of each trade's set here.
        let got = |k: &str| -> BTreeSet<&str> {
            rules.iter().filter(|r| r.names_kinds() && r.id != "G-PRODUCT" && r.applies_to_kind(k)).map(|r| r.id.as_str()).collect()
        };
        let product = |k: &str| rules.iter().any(|r| r.id == "G-PRODUCT" && r.applies_to_kind(k));
        for k in ["plumbing_fixture", "light_fixture", "gfci_receptacle", "ptac", "exhaust_fan", "occupancy_sensor", "smoke_detector"] {
            assert!(product(k), "{k} has a product record to check");
        }
        for k in ["return_grille", "wall", "duct", "equipment_connection"] {
            assert!(!product(k), "{k} is no product G-PRODUCT checks");
        }
        let set = |ids: &[&'static str]| -> BTreeSet<&'static str> { ids.iter().copied().collect() };
        assert_eq!(got("plumbing_fixture"), set(&["P-FIXTURE"]));
        assert!(!got("light_fixture").contains("P-FIXTURE"), "a light fixture is no plumbing fixture");
        for k in ["return_grille", "exhaust_grille", "diffuser", "supply_diffuser"] {
            assert_eq!(got(k), set(&["M-TERMINAL"]), "{k}");
        }
        // Fire alarm devices: host, and their own rule (present, positions, heights).
        assert_eq!(got("fire_alarm_device"), set(&["G-HOST", "E-FA"]));
        assert_eq!(got("horn_strobe"), set(&["G-HOST", "E-FA"]));
        assert_eq!(got("smoke_detector"), set(&["G-HOST", "E-FA"]));
        // Through-wall units: present, in an exterior wall.
        assert_eq!(got("ptac"), set(&["M-PTAC"]));
        assert_eq!(got("through_wall_unit"), set(&["M-PTAC"]));
        // Exhaust fans: present, in a ceiling; and the connection of a fan the tables don't name.
        assert_eq!(got("exhaust_fan"), set(&["M-EF"]));
        assert_eq!(got("equipment_connection"), set(&["E-CIRCUIT", "E-EQUIP-CONN", "E-EQUIP-ELEC"]));
        // Occupancy sensors: present, host; and as lighting controls, their switch leg, a dimming
        // one's drivers and the room's count (Q6145).
        let sensor = set(&["G-HOST", "E-OCC", "E-SWITCH-LEG", "E-SWITCH-DIM", "E-LUM-COUNT"]);
        assert_eq!(got("occupancy_sensor"), sensor);
        assert_eq!(got("vacancy_sensor"), sensor);
        // A conduit is hosted by nothing; G-HOST is not its rule.
        assert!(!got("conduit").contains("G-HOST"));
        let rule = |id: &str| rules.iter().find(|r| r.id == id).unwrap();
        for (id, words) in [
            ("E-FA", &["in the model", "ceiling", "height"][..]),
            ("M-PTAC", &["exterior wall"][..]),
            ("M-EF", &["ceiling", "in the model"][..]),
            ("E-OCC", &["in the model", "wall", "ceiling"][..]),
        ] {
            for w in words {
                assert!(rule(id).check.contains(w) || rule(id).how.iter().any(|h| h.contains(w)), "{id}: {w}");
            }
        }
    }

    /// Kinds Validate round 2 found without a proper rule (Q6140): a wall luminaire gets the
    /// luminaire and circuit rules, a thermostat its own rule beside host, height and present, a
    /// fan coil unit the mechanical equipment rule and not the exhaust fan's.
    #[test]
    fn leftover_kinds_get_proper_rules() {
        let rules = builtin().unwrap();
        let got = |k: &str| -> BTreeSet<&str> { rules.iter().filter(|r| r.applies_to_kind(k)).map(|r| r.id.as_str()).collect() };
        let lum = got("wall_luminaire");
        for id in ["E-LUM", "E-CIRCUIT", "E-LUM-TYPE", "E-LUM-CONTROL", "E-LUM-COUNT", "G-HEIGHT", "G-PRESENT"] {
            assert!(lum.contains(id), "wall_luminaire: {id} in {lum:?}");
        }
        assert!(!lum.iter().any(|r| r.starts_with("M-") || r.starts_with("P-")), "{lum:?}");
        let ts = got("thermostat");
        for id in ["M-TSTAT", "G-HOST", "G-HEIGHT", "G-PRESENT"] {
            assert!(ts.contains(id), "thermostat: {id} in {ts:?}");
        }
        assert!(!ts.iter().any(|r| r.starts_with("E-")), "a thermostat is no electrical device: {ts:?}");
        let rule = |id: &str| rules.iter().find(|r| r.id == id).unwrap();
        assert_eq!(rule("M-TSTAT").trade, "mechanical");
        for w in ["in the model", "wall", "height"] {
            assert!(rule("M-TSTAT").check.contains(w), "{w}");
        }
        for k in ["fan_coil_unit", "Fan Coil Unit", "fcu"] {
            let fcu = got(k);
            assert!(fcu.contains("M-EQUIP") && !fcu.contains("M-EF"), "{k}: {fcu:?}");
        }
        // By its category too: a fan coil unit is written as unitary equipment, which M-EF doesn't name.
        let cat = super::super::validate_kind::category_of_kind("fan_coil_unit");
        assert_eq!(cat.as_deref(), Some("IfcUnitaryEquipment"));
        assert!(!rule("M-EF").applies_to_element("fan_coil_unit", cat.as_deref(), Some("FCU-1")));
        assert!(rule("M-EQUIP").applies_to_element("fan_coil_unit", cat.as_deref(), Some("FCU-1")));
        // An exhaust fan is still M-EF's.
        assert!(got("exhaust_fan").contains("M-EF") && !got("exhaust_fan").contains("M-EQUIP"));
    }

    #[test]
    fn an_element_gets_a_rule_by_kind_category_or_mark() {
        let rules = builtin().unwrap();
        let rule = |id: &str| rules.iter().find(|r| r.id == id).unwrap();
        let host = rule("G-HOST");
        assert!(!host.applies_to_element("device", None, Some("REC")), "a generic device with a mark names nothing");
        assert!(host.applies_to_element("device", Some("IfcOutlet"), None));
        assert!(host.applies_to_element("gfci_receptacle", None, None));
        assert!(rule("M-TERMINAL").applies_to_element("fixture", Some("ifcairterminal"), None));
        let mut ef = rule("M-EF").clone();
        ef.applies_to.marks = vec!["EF-*".into()];
        assert!(ef.applies_to_element("device", None, Some("ef-2")) && !ef.applies_to_element("device", None, Some("REF-2")));
        assert!(host.names_kinds() && !rule("G-HEIGHT").names_kinds());
    }

    #[test]
    fn mark_patterns_match_any_case_with_stars() {
        for (p, m) in [("EF-*", "EF-12"), ("ef-*", "EF-"), ("*-A", "LP-A"), ("L*-*2", "LP-1A2"), ("REC", "rec"), ("*", "X")] {
            assert!(mark_matches(p, m), "{p} {m}");
        }
        for (p, m) in [("EF-*", "REF-1"), ("*-A", "LP-B"), ("REC", "REC2"), ("L*-*2", "LP-1A"), ("", "A"), ("A*", ""), ("AB*B", "AB"), ("É*", "E")] {
            assert!(!mark_matches(p, m), "{p} {m}");
        }
    }

    #[test]
    fn a_marks_kind_is_no_rule_override() {
        assert!(!is_override_key("validate.kind.REC") && is_override_key("validate.G-HOST.enabled"));
        let mut pr = ProjectRules::default();
        pr.set(RuleDomain::Custom, "validate.kind.REC", RuleValue::new("receptacle"), "the legend");
        let (_, rejected) = effective(&pr).unwrap();
        assert!(rejected.is_empty(), "{rejected:?}");
    }

    #[test]
    fn the_command_lists_filters_and_pages() {
        let mut s = session();
        let all = s.execute(CMD_RULES, &json!({"limit": 50})).unwrap();
        let n = all["total"].as_u64().unwrap() as usize;
        assert_eq!(n, builtin().unwrap().len());
        let r = &all["rules"][0];
        for f in ["id", "trade", "applies_to", "check", "how", "documents", "tolerance", "severity", "prescreen", "examples", "enabled", "source"] {
            assert!(r.get(f).is_some(), "{f}");
        }
        assert_eq!(r["source"], "built-in");

        let mech = s.execute(CMD_RULES, &json!({"trade": "mechanical"})).unwrap();
        assert!(mech["rules"].as_array().unwrap().iter().all(|r| r["trade"] == "mechanical"));
        let one = s.execute(CMD_RULES, &json!({"id": "m-ef"})).unwrap();
        assert_eq!(one["total"], 1);
        assert_eq!(one["rules"][0]["id"], "M-EF");
        let fan = s.execute(CMD_RULES, &json!({"kind": "exhaust_fan", "limit": 50})).unwrap();
        let ids: Vec<&str> = fan["rules"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
        assert!(ids.contains(&"M-EF") && ids.contains(&"G-ROOM"), "{ids:?}");
        assert_eq!(fan["kind_known"], true);

        // Paging walks every rule once.
        let mut seen = Vec::new();
        let mut cursor = 0;
        for _ in 0..100 {
            let pg = s.execute(CMD_RULES, &json!({"cursor": cursor, "limit": 4})).unwrap();
            seen.extend(pg["rules"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str().map(str::to_string)));
            match pg["next"].as_u64() {
                Some(c) => cursor = c as usize,
                None => break,
            }
        }
        assert_eq!(seen.len(), n);
        assert_eq!(seen.iter().collect::<BTreeSet<_>>().len(), n);

        for bad in [json!({"trade": "dance"}), json!({"id": "NOPE-1"}), json!({"limit": -1}), json!({"detail": "yes"})] {
            assert!(s.execute(CMD_RULES, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_answer_stays_under_answer_max() {
        let mut s = session();
        for p in [json!({"limit": 50}), json!({"limit": 50, "detail": false}), json!({"kind": "receptacle", "limit": 50}), json!({})] {
            let mut cursor = 0usize;
            let mut total_seen = 0usize;
            for _ in 0..100 {
                let mut q = p.clone();
                q["cursor"] = json!(cursor);
                let v = s.execute(CMD_RULES, &q).unwrap();
                assert!(v.to_string().len() <= ANSWER_MAX, "{p}: {}", v.to_string().len());
                total_seen += v["rules"].as_array().unwrap().len();
                match v["next"].as_u64() {
                    Some(c) => cursor = c as usize,
                    None => break,
                }
            }
            assert_eq!(total_seen as u64, s.execute(CMD_RULES, &p).unwrap()["total"].as_u64().unwrap(), "{p}");
        }
    }

    #[test]
    fn overrides_change_tolerance_and_can_disable_a_rule() {
        let mut s = session();
        let r = s.execute(CMD_SET, &json!({"id": "G-PLACE", "tolerance": {"plan_in": 3}, "severity": "high", "reason": "tight fit-out"})).unwrap();
        assert_eq!(r["rule"]["tolerance"]["plan_in"], 3.0);
        assert_eq!(r["rule"]["severity"], "high");
        assert_eq!(r["rule"]["source"], "project");
        s.execute(CMD_SET, &json!({"id": "F-HEAD", "enabled": false, "reason": "sprinklers by others"})).unwrap();

        let g = s.execute(CMD_RULES, &json!({"id": "G-PLACE"})).unwrap();
        let rule = &g["rules"][0];
        assert_eq!(rule["tolerance"]["plan_in"], 3.0);
        let ov = rule["overrides"].as_array().unwrap();
        assert!(ov.iter().any(|o| o["field"] == "tolerance.plan_in" && o["built_in"] == 6.0 && o["project"] == 3.0), "{ov:?}");
        let f = s.execute(CMD_RULES, &json!({"id": "F-HEAD"})).unwrap();
        assert_eq!(f["rules"][0]["enabled"], false);
        assert_eq!(f["rules"][0]["source"], "project");
        // Untouched rules stay built-in.
        assert_eq!(s.execute(CMD_RULES, &json!({"id": "M-EF"})).unwrap()["rules"][0]["source"], "built-in");

        // The same through project.rules.set.
        s.execute(
            "project.rules.set",
            &json!({"domain": "tolerance", "key": "validate.M-DUCT-ROUTE.tolerance.route_ft", "value": "2", "reason": "loose"}),
        )
        .unwrap();
        let m = s.execute(CMD_RULES, &json!({"id": "M-DUCT-ROUTE"})).unwrap();
        assert_eq!(m["rules"][0]["tolerance"]["route_ft"], 2.0);

        // Reset puts the rule back.
        s.execute(CMD_SET, &json!({"id": "G-PLACE", "reset": true, "reason": "back to standard"})).unwrap();
        let g = s.execute(CMD_RULES, &json!({"id": "G-PLACE"})).unwrap();
        assert_eq!(g["rules"][0]["tolerance"]["plan_in"], 6.0);
        assert_eq!(g["rules"][0]["source"], "built-in");
    }

    #[test]
    fn bad_overrides_are_refused_or_listed() {
        let mut s = session();
        for bad in [
            json!({"id": "G-PLACE", "tolerance": {"plan_in": -1}, "reason": "x"}),
            json!({"id": "G-PLACE", "tolerance": {"furlongs": 1}, "reason": "x"}),
            json!({"id": "G-PLACE", "severity": "urgent", "reason": "x"}),
            json!({"id": "NOPE", "enabled": false, "reason": "x"}),
            json!({"id": "G-PLACE", "enabled": false}),
            json!({"id": "G-PLACE", "reason": "x"}),
        ] {
            assert!(s.execute(CMD_SET, &bad).is_err(), "{bad}");
        }
        // Written straight into the project rules (or imported), a bad value is not used and says why.
        s.execute("project.rules.set", &json!({"domain": "custom", "key": "validate.G-PLACE.severity", "value": "urgent", "reason": "x"})).ok();
        s.doc_mut().unwrap().project_rules.set(RuleDomain::Custom, "validate.G-PLACE.severity", RuleValue::new("urgent"), "imported");
        s.doc_mut().unwrap().project_rules.set(RuleDomain::Custom, "validate.NOPE.enabled", RuleValue::new("false"), "imported");
        let v = s.execute(CMD_RULES, &json!({"id": "G-PLACE"})).unwrap();
        assert_eq!(v["rules"][0]["severity"], "medium");
        let rej = v["rejected"].as_array().unwrap();
        assert!(rej.iter().any(|r| r["key"] == "custom/validate.G-PLACE.severity"), "{rej:?}");
        assert!(rej.iter().any(|r| r["why"].as_str().unwrap().contains("NOPE")), "{rej:?}");
        // A tolerance in the wrong domain is named.
        let e = check_override(RuleDomain::Custom, "validate.G-PLACE.tolerance.plan_in", "3").unwrap_err();
        assert!(e.contains("domain `tolerance`"), "{e}");
    }

    #[test]
    fn project_rules_set_refuses_a_bad_validation_override() {
        let mut s = session();
        let e = s
            .execute("project.rules.set", &json!({"domain": "custom", "key": "validate.G-PLACE.severity", "value": "urgent", "reason": "x"}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("high, medium or low"), "{e}");
        // Other custom keys are untouched by the check.
        s.execute("project.rules.set", &json!({"domain": "custom", "key": "validated_by", "value": "anyone", "reason": "x"})).unwrap();
    }
}
