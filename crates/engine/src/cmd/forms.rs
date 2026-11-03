//! Object forms (`bim.forms`, `bim.form.*`, `bim.completeness`): what every kind of object must
//! say, and how much of it the model knows.
//!
//! A FORM is a kind's list of fields: each with its type, unit, allowed values, whether it is
//! required, and an optional condition (`protected_by` is required only where `gfci` is not
//! `none`). Forms come from STANDARD TEMPLATES — shipped rules data, one file per trade
//! (`forms/*.json`), editable: a file of the same shape in the settings folder's `forms/` folder
//! replaces a kind's template — or, for a kind no template covers, a minimal GENERIC form (kind,
//! mark, location, host, level, room, source).
//!
//! The job makes the PROJECT forms after the model build ([`job_pass`]): one per kind the model
//! holds and the print inventory found. A project form overrides its template and GROWS as the
//! model is built: a value read from a legend, schedule, spec or submittal under a key the form
//! lacks adds that field (with its source and author), and agents add fields, mark a field N/A
//! for the kind with a reason, or require it (`bim.form.field`). Every change is logged (who,
//! when, why, source) and can be reverted (`bim.form.revert`); a project form can be promoted
//! back to a template (`bim.form.promote`).
//!
//! Every value of a field has a STATUS: `documented` (read from a sheet, legend, schedule, spec,
//! submittal or note, or entered by a person), `derived` (a named rule or computed from the
//! model), `default` (an assumption: it does NOT count as filled), `conflict` (sources disagree:
//! a `conflict` review item), `missing`, or `n/a` (with the reason). `bim.form.record` is how a
//! job step or an agent writes a value it read; a second document that disagrees with a
//! documented value becomes a conflict instead of overwriting it.
//!
//! `bim.completeness` gives the fill rate (documented + derived over required) per kind × field ×
//! level, the lowest required fields as the next work, and, given an earlier answer as
//! `baseline`, a gate that fails when a kind's required-field fill drops.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadaddy_doc::props::{self, Entry, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity};
use cadaddy_walk3d::validate_kinds;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{EngineError, Result, Session};

const FORMS: &str = "bim.forms";
const CREATE: &str = "bim.form.create";
const FIELD: &str = "bim.form.field";
const RECORD: &str = "bim.form.record";
const LOG: &str = "bim.form.log";
const REVERT: &str = "bim.form.revert";
const PROMOTE: &str = "bim.form.promote";
const COMPLETENESS: &str = "bim.completeness";

/// Where the drawing keeps the project forms, their log and the open conflicts.
const FILE: &str = "object-forms.json";

/// The shipped templates, one file per trade.
const TRADE_FILES: [&str; 7] = [
    include_str!("forms/electrical.json"),
    include_str!("forms/lighting.json"),
    include_str!("forms/lowvoltage.json"),
    include_str!("forms/mechanical.json"),
    include_str!("forms/plumbing.json"),
    include_str!("forms/architectural.json"),
    include_str!("forms/structural.json"),
];

/// Most project forms, fields per form, log entries and conflicts kept.
const MAX_FORMS: usize = 5_000;
const MAX_FIELDS: usize = 200;
const MAX_LOG: usize = 20_000;
const MAX_CONFLICTS: usize = 20_000;
/// Longest key, kind, reason or note kept.
const MAX_KEY: usize = 64;
const MAX_KIND: usize = 120;
const MAX_TEXT: usize = 400;
/// Nesting of a condition evaluated at most.
const MAX_COND_DEPTH: usize = 8;
/// Elements one `bim.form.record` writes at most.
const MAX_RECORD: usize = 100_000;
/// Lowest-fill required fields listed as the next work.
const NEXT_WORK: usize = 15;
/// A template file read from the settings folder, at most.
const MAX_TEMPLATE_FILE: u64 = 2 * 1024 * 1024;
/// Keys the readers keep for their own bookkeeping: never a field of a form.
const BOOKKEEPING: [&str; 7] = ["kind", "kind_source", "ifc_class", "identity", "seen_on", "fed_from_handle", "unresolved"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(FORMS, "Object Forms", run_forms)
            .params("{kind?, templates?: bool (the standard templates instead of the project's forms)} → {forms: [{kind, template, trade, label, fields: [{key, type, unit?, allowed?, required, when?, aliases?, note?, origin, source?, author?, na?}]}], templates: [kind], project: bool}: each kind's form; with no project forms yet, the templates")
            .noundo(),
        CommandSpec::new(CREATE, "Create Object Forms", run_create)
            .params("{kinds?: [kind] (default: every kind the model holds and the print inventory found), author?, grow?: bool (default true: add the fields the documents give that a form lacks)} → {created: [{kind, from: template kind | generic}], grown: [{kind, key, source}], forms}: the project forms, from the standard templates (a generic form where none fits)"),
        CommandSpec::new(FIELD, "Edit Object Form Field", run_field)
            .params("{kind, key, action: add | na | applies | require | optional | remove | edit, reason? (required for na), type?: text|number|bool|enum|ref, unit?, allowed?: [value], required?: bool, when?: {key, in?: [..], not_in?: [..], exists?: bool} | {any|all: [cond]}, aliases?: [key], note?, source?, author?} → {kind, key, change, field}: a project form's field changed; logged and revertible"),
        CommandSpec::new(RECORD, "Record Object Field Value", run_record)
            .params("{handle? | handles? (default: the selection), key, value, unit?, source: {kind: print|legend|schedule|spec|submittal|note|rule|computed, doc?, at?: [x, y]} | kind, author?, note?} → {set, conflicts: [{id, handle, key, values}], added_fields: [{kind, key}]}: a value a job step or agent read from a document; a different document's value already there makes a conflict (a review item) instead of overwriting it; a key the kind's form lacks is added to it"),
        CommandSpec::new(LOG, "Object Form Log", run_log)
            .params("{kind?, limit? (default 50)} → {changes: [{id, at, who, why, source, kind, key, action, reverted}], total}")
            .noundo(),
        CommandSpec::new(REVERT, "Revert Object Form Change", run_revert)
            .params("{change: id, author?} → {reverted, kind, key}: the form as it was before that change (a later change of the same field is reverted first)"),
        CommandSpec::new(PROMOTE, "Promote Object Form to Template", run_promote)
            .params("{kind, folder? (default: the settings folder's forms/)} → {path, kind, fields}: the project form written as that kind's template (it replaces the shipped one on this machine)"),
        CommandSpec::new(COMPLETENESS, "Object Completeness", run_completeness)
            .params("{kind?, field?, level?, room?, baseline?: an earlier bim.completeness answer, tolerance_pct? (default 0)} → {elements, overall: {required, filled, documented, pct, documented_pct}, kinds: {kind: {count, form, required, filled, required_fill_pct, documented_pct, fields: {key: {required, filled, documented, derived, default, conflict, missing, na, pct, documented_pct, by_level: {level: pct}}}}}, next_work: [text], gate?: {pass, drops: [{kind, before, after}]}}: fill rates (documented + derived; defaults unfilled, n/a not required)")
            .noundo(),
    ]
}

// ─── Forms and templates ─────────────────────────────────────────────────────

/// A condition on an element's other fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cond {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, rename = "in", skip_serializing_if = "Vec::is_empty")]
    pub is_in: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_in: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<Cond>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<Cond>,
}

impl Cond {
    /// Whether the element's values (`get`: a key → its value as text) meet it.
    fn holds(&self, get: &dyn Fn(&str) -> Option<String>, depth: usize) -> bool {
        if depth > MAX_COND_DEPTH {
            return false;
        }
        let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        let mut ok = true;
        if let Some(k) = &self.key {
            let v = get(k);
            if !self.is_in.is_empty() {
                ok &= v.as_deref().is_some_and(|v| self.is_in.iter().any(|x| same(x, v)));
            }
            if !self.not_in.is_empty() {
                ok &= v.as_deref().is_some_and(|v| !self.not_in.iter().any(|x| same(x, v)));
            }
            if let Some(e) = self.exists {
                ok &= v.is_some() == e;
            }
            if self.is_in.is_empty() && self.not_in.is_empty() && self.exists.is_none() {
                ok &= v.is_some();
            }
        }
        if !self.any.is_empty() {
            ok &= self.any.iter().take(64).any(|c| c.holds(get, depth + 1));
        }
        if !self.all.is_empty() {
            ok &= self.all.iter().take(64).all(|c| c.holds(get, depth + 1));
        }
        ok
    }

    /// How a person reads it.
    fn describe(&self, depth: usize) -> String {
        if depth > MAX_COND_DEPTH {
            return "…".into();
        }
        let mut parts = Vec::new();
        if let Some(k) = &self.key {
            if !self.is_in.is_empty() {
                parts.push(format!("{k} is {}", self.is_in.join(" or ")));
            }
            if !self.not_in.is_empty() {
                parts.push(format!("{k} is given and not {}", self.not_in.join(" or ")));
            }
            match self.exists {
                Some(true) => parts.push(format!("{k} is given")),
                Some(false) => parts.push(format!("{k} is not given")),
                None if self.is_in.is_empty() && self.not_in.is_empty() => parts.push(format!("{k} is given")),
                None => {}
            }
        }
        if !self.any.is_empty() {
            parts.push(format!("({})", self.any.iter().take(16).map(|c| c.describe(depth + 1)).collect::<Vec<_>>().join(" or ")));
        }
        if !self.all.is_empty() {
            parts.push(format!("({})", self.all.iter().take(16).map(|c| c.describe(depth + 1)).collect::<Vec<_>>().join(" and ")));
        }
        parts.join(" and ")
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn text_type() -> String {
    "text".into()
}

/// One field of a form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    #[serde(rename = "type", default = "text_type")]
    pub ty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Cond>,
    /// Other property keys that hold the same value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// `template`, `generic` or `added` (a project form's own field).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// Where an added field came from (the document that gave it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Who added it (a job step or an agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Not applicable to this kind on this project: why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub na: Option<String>,
}

impl Field {
    /// Whether property `key` is this field (its key or one of its aliases).
    fn names(&self, key: &str) -> bool {
        self.key.eq_ignore_ascii_case(key) || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(key))
    }
}

/// A standard template: a kind's fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trade: String,
}

/// A trade's template file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TradeFile {
    trade: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    about: String,
    templates: Vec<Template>,
}

/// A kind's form on this project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Form {
    pub kind: String,
    /// The template it came from (its kind), or `generic`.
    pub template: String,
    #[serde(default)]
    pub trade: String,
    #[serde(default)]
    pub label: String,
    pub fields: Vec<Field>,
}

impl Form {
    fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key.eq_ignore_ascii_case(key)).or_else(|| self.fields.iter().find(|f| f.names(key)))
    }
}

/// One change of a project form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub id: u64,
    /// Seconds since 1970.
    pub at: u64,
    pub who: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// `create`, `add`, `na`, `applies`, `require`, `optional`, `remove`, `edit`, `promote`,
    /// `revert`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Field>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverted: bool,
}

/// One value a source gave for a field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Given {
    pub value: Value,
    pub source: Source,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// Sources that disagree about one element's field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub handle: String,
    pub kind: String,
    pub key: String,
    pub values: Vec<Given>,
    #[serde(default)]
    pub open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
}

/// What the drawing keeps.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub forms: BTreeMap<String, Form>,
    #[serde(default)]
    pub log: Vec<Change>,
    #[serde(default)]
    pub next: u64,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
}

/// The project forms the drawing keeps (none yet: empty).
pub fn stored(d: &Drawing) -> Store {
    d.embedded.get(FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
}

fn save(s: &mut Session, st: &Store) -> Result<()> {
    let bytes = serde_json::to_vec(st).map_err(|e| EngineError::Other(format!("{FORMS}: {e}")))?;
    s.doc_mut()?.embedded.insert(FILE.into(), Arc::new(bytes));
    s.touch();
    Ok(())
}

fn now() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn cap(t: &str, n: usize) -> String {
    t.trim().chars().take(n).collect()
}

impl Store {
    fn log(&mut self, mut c: Change) -> u64 {
        self.next = self.next.saturating_add(1);
        c.id = self.next;
        c.at = now();
        let id = c.id;
        self.log.push(c);
        if self.log.len() > MAX_LOG {
            let extra = self.log.len() - MAX_LOG;
            self.log.drain(..extra);
        }
        id
    }

    /// The project form of `kind`, made from its template (or the generic form) if there is none
    /// yet; the creation is logged.
    fn ensure(&mut self, kind: &str, templates: &[Template], who: &str, why: &str) -> Result<&mut Form> {
        if !self.forms.contains_key(kind) {
            if self.forms.len() >= MAX_FORMS {
                return Err(EngineError::Other(format!("{FORMS}: the drawing holds {MAX_FORMS} forms, the most it keeps")));
            }
            let form = new_form(kind, templates);
            self.log(Change {
                who: who.into(),
                why: why.into(),
                source: if form.template == GENERIC { "the generic form".into() } else { format!("template {} ({})", form.template, form.trade) },
                kind: kind.into(),
                action: "create".into(),
                ..Change::default()
            });
            self.forms.insert(kind.to_string(), form);
        }
        self.forms.get_mut(kind).ok_or_else(|| EngineError::Other(format!("{FORMS}: no form for {kind}")))
    }
}

/// The generic form's template name.
const GENERIC: &str = "generic";

/// The minimal form of a kind no template covers.
fn generic_fields() -> Vec<Field> {
    let f = |key: &str, required: bool, aliases: &[&str], note: &str| Field {
        key: key.into(),
        ty: "text".into(),
        required,
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
        note: Some(note.to_string()).filter(|n| !n.is_empty()),
        origin: GENERIC.into(),
        ..Field::default()
    };
    vec![
        f("kind", true, &[], "what it is"),
        f("mark", true, &["type_mark", "type", "tag", "device_type"], "its mark or type tag on the prints"),
        f("location", true, &[], ""),
        f("host", false, &["host_wall"], "the wall, ceiling or floor it is in or on"),
        f("level", true, &[], ""),
        f("room", true, &[], ""),
        f("source", true, &["sheet", "source_sheet"], "the sheet and point it was read from"),
    ]
}

fn new_form(kind: &str, templates: &[Template]) -> Form {
    match template_for(templates, kind) {
        Some(t) => Form {
            kind: kind.into(),
            template: t.kind.clone(),
            trade: t.trade.clone(),
            label: t.label.clone(),
            fields: t.fields.iter().take(MAX_FIELDS).map(|f| Field { origin: "template".into(), ..f.clone() }).collect(),
        },
        None => Form { kind: kind.into(), template: GENERIC.into(), trade: String::new(), label: String::new(), fields: generic_fields() },
    }
}

/// The templates: the shipped ones, each kind replaced by the settings folder's `forms/*.json`.
pub fn templates() -> Vec<Template> {
    let mut out: Vec<Template> = TRADE_FILES.iter().flat_map(|t| parse_trade(t)).collect();
    if let Some(dir) = user_folder(None) {
        for t in read_folder(&dir) {
            out.retain(|x| !x.kind.eq_ignore_ascii_case(&t.kind));
            out.push(t);
        }
    }
    out
}

fn parse_trade(text: &str) -> Vec<Template> {
    let Ok(f) = serde_json::from_str::<TradeFile>(text) else { return Vec::new() };
    f.templates.into_iter().take(MAX_FORMS).map(|t| clean_template(t, &f.trade)).collect()
}

fn clean_template(mut t: Template, trade: &str) -> Template {
    t.kind = norm_kind(&t.kind);
    trade.clone_into(&mut t.trade);
    t.fields.truncate(MAX_FIELDS);
    for f in &mut t.fields {
        f.key = cap(&f.key, MAX_KEY);
        f.aliases.truncate(16);
    }
    t.fields.retain(|f| !f.key.is_empty());
    t
}

/// The folder of the user's templates: `folder` given, else the settings folder's `forms/`.
fn user_folder(folder: Option<&str>) -> Option<std::path::PathBuf> {
    match folder.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => Some(std::path::PathBuf::from(f)),
        None => crate::control_auth::config_dir().map(|d| d.join("forms")),
    }
}

fn read_folder(dir: &std::path::Path) -> Vec<Template> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<std::path::PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).take(64).collect();
    files.sort();
    files
        .into_iter()
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_TEMPLATE_FILE))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|t| parse_trade(&t))
        .collect()
}

/// Lower-case words joined by `_`; a printinventory `legend:<words>` kind keeps its prefix and
/// words.
pub fn norm_kind(kind: &str) -> String {
    let t = kind.trim();
    if t.len() > 7 && t.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("legend:")) {
        let words: Vec<String> = t.get(7..).unwrap_or("").split_whitespace().map(str::to_ascii_lowercase).collect();
        return cap(&format!("legend:{}", words.join(" ")), MAX_KIND);
    }
    let w: String = t.chars().take(MAX_KIND).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    w.split('_').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("_")
}

/// The words of a kind, `_`-joined, for matching (a `legend:` kind's row words).
fn kind_words(kind: &str) -> String {
    let k = norm_kind(kind);
    let k = k.strip_prefix("legend:").map_or(k.clone(), str::to_string);
    norm_kind(&k.replace(' ', "_"))
}

/// The template a kind takes: one naming it (its kind or an alias), else the most specific one
/// whose family holds it (`gfci_receptacle` → `receptacle`; `legend:recessed downlight` →
/// `luminaire`), else one whose kind or alias its words hold.
pub fn template_for<'a>(ts: &'a [Template], kind: &str) -> Option<&'a Template> {
    let k = kind_words(kind);
    if k.is_empty() {
        return None;
    }
    let names = |t: &Template| std::iter::once(t.kind.clone()).chain(t.aliases.iter().map(|a| norm_kind(a))).collect::<Vec<_>>();
    if let Some(t) = ts.iter().find(|t| names(t).contains(&k)) {
        return Some(t);
    }
    let fam: Vec<&Template> = ts.iter().filter(|t| names(t).iter().any(|n| validate_kinds::kind_matches(n, &k))).collect();
    if !fam.is_empty() {
        // The narrowest: the one inside the most of the others' families.
        return fam.iter().copied().max_by_key(|t| {
            (fam.iter().filter(|o| o.kind != t.kind && validate_kinds::in_family(&t.kind, &o.kind)).count(), std::cmp::Reverse(t.kind.clone()))
        });
    }
    let w = format!("_{k}_");
    ts.iter()
        .filter_map(|t| names(t).into_iter().filter(|n| w.contains(&format!("_{n}_"))).map(|n| n.len()).max().map(|l| (l, t)))
        .max_by_key(|(l, t)| (*l, std::cmp::Reverse(t.kind.clone())))
        .map(|(_, t)| t)
}

// ─── An element's values and statuses ────────────────────────────────────────

/// The kind of form an element takes: a device's own kind (`receptacle`), a scheduled
/// equipment's or terminal's kind (`ptac`, `grille`), else the model's kind.
pub fn form_kind(e: &Entity, base: String) -> String {
    let k = super::elemprops::reported_kind(e, base);
    if matches!(k.as_str(), "equipment" | "fixture" | "device")
        && let Some(part) = super::bimlook::made_tag(e, "part")
    {
        let p = part.trim().to_ascii_lowercase();
        if let Some(x) = p.strip_prefix("equipment:").or_else(|| p.strip_prefix("terminal:")).filter(|x| !x.trim().is_empty()) {
            return norm_kind(x);
        }
    }
    norm_kind(&k)
}

/// A value's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Documented,
    Derived,
    Default,
    Conflict,
    Missing,
    Na,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Documented => "documented",
            Status::Derived => "derived",
            Status::Default => "default",
            Status::Conflict => "conflict",
            Status::Missing => "missing",
            Status::Na => "n/a",
        }
    }

    pub fn filled(self) -> bool {
        matches!(self, Status::Documented | Status::Derived)
    }
}

/// The status a stored value has by itself, and why when it says so.
fn value_status(p: &Prop) -> (Status, Option<String>) {
    let t = p.text().trim().to_ascii_lowercase();
    let note = p.note.clone().filter(|n| !n.trim().is_empty());
    match t.as_str() {
        "n/a" | "na" | "not applicable" => return (Status::Na, Some(note.unwrap_or_else(|| "marked not applicable".into()))),
        "undecided" => {
            let n = note.clone().unwrap_or_default().to_ascii_lowercase();
            return if n.contains("conflict") || n.contains("disagree") {
                (Status::Conflict, note)
            } else {
                (Status::Missing, Some(note.unwrap_or_else(|| "undecided".into())))
            };
        }
        "not stated in the documents" | "unknown" | "?" => return (Status::Missing, Some(note.unwrap_or_else(|| p.text()))),
        _ => {}
    }
    if note.as_deref().is_some_and(|n| n.trim().to_ascii_lowercase().starts_with("conflict")) {
        return (Status::Conflict, note);
    }
    match p.source.kind {
        SourceKind::Default => (Status::Default, None),
        SourceKind::Rule | SourceKind::Computed => (Status::Derived, None),
        _ => (Status::Documented, None),
    }
}

/// An element's values: its properties (stored and computed) and what the model says of it
/// (its kind, where it is, the sheet object it came from).
pub fn values(d: &Drawing, e: &Entity, base: &str, kind: &str, upf: f64) -> Vec<(String, Prop)> {
    let mut out: Vec<(String, Prop)> =
        super::elemprops::effective(e, base, upf).into_iter().filter(|(_, p, _)| p.has_value()).map(|(k, p, _)| (k, p)).collect();
    let has = |out: &[(String, Prop)], k: &str| out.iter().any(|(q, _)| q.eq_ignore_ascii_case(k));
    if !has(&out, "kind") {
        out.push(("kind".into(), Prop::new(kind.to_string(), Source::computed())));
    }
    if !has(&out, "location") {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if !b.is_empty() && upf > 0.0 {
            let c = b.center();
            out.push(("location".into(), Prop::new(format!("{:.2}, {:.2}", c.x / upf, c.y / upf), Source::computed()).with_unit("ft")));
        }
    }
    // What the model measures of it: a run's length, a ceiling's or floor's area.
    if (!has(&out, "length") || !has(&out, "area"))
        && let Some(c) = super::bimlook::classify(d, e)
    {
        let q = super::bimlook::quantity(&c, e, upf);
        for (key, qk, unit) in [("length", "length_ft", "ft"), ("area", "area_sf", "sf")] {
            if !has(&out, key)
                && let Some(v) = q.get(qk).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0)
            {
                out.push((key.into(), Prop::new(v, Source::computed()).with_unit(unit)));
            }
        }
    }
    if !has(&out, "source")
        && let Some(h) = super::bimlook::made_source(e)
    {
        out.push(("source".into(), Prop::new(format!("sheet object {}", h.hex()), Source::of(SourceKind::Print))));
    }
    out
}

/// One field of one element: its value, status and source.
#[derive(Clone, Debug)]
pub struct FieldState {
    pub key: String,
    pub required: bool,
    pub status: Status,
    pub value: Option<Prop>,
    /// The property key the value is under, when it is an alias.
    pub from_key: Option<String>,
    pub reason: Option<String>,
    pub check: Option<String>,
}

/// Each field of `form` for an element with `vals`, its open conflicts by key.
pub fn field_states(form: &Form, vals: &[(String, Prop)], conflicts: &BTreeSet<String>) -> Vec<FieldState> {
    let find = |f: &Field| -> Option<(String, Prop)> {
        vals.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&f.key))
            .or_else(|| vals.iter().find(|(k, _)| f.names(k)))
            .map(|(k, p)| (k.clone(), p.clone()))
    };
    // A condition reads the element's fields by key or alias, then its raw properties.
    let get = |key: &str| -> Option<String> {
        form.field(key)
            .and_then(find)
            .or_else(|| vals.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(k, p)| (k.clone(), p.clone())))
            .map(|(_, p)| p.text())
            .filter(|t| !t.trim().is_empty())
    };
    form.fields
        .iter()
        .map(|f| {
            let found = find(f);
            let applies_kind = f.na.is_none();
            let applies_here = f.when.as_ref().is_none_or(|c| c.holds(&get, 0));
            let required = f.required && applies_kind && applies_here;
            let (mut status, mut reason) = match &found {
                Some((_, p)) => value_status(p),
                None => (Status::Missing, None),
            };
            if conflicts.iter().any(|k| f.names(k)) {
                status = Status::Conflict;
                reason = Some("the documents disagree (a conflict review item)".into());
            }
            if status == Status::Missing && !(applies_kind && applies_here) {
                status = Status::Na;
                reason = Some(match (&f.na, &f.when) {
                    (Some(r), _) => r.clone(),
                    (None, Some(c)) => format!("required only where {}", c.describe(0)),
                    _ => "not applicable".into(),
                });
            }
            let check = match &found {
                Some((_, p)) if f.ty == "enum" && !f.allowed.is_empty() && status.filled() => {
                    let t = p.text();
                    (!f.allowed.iter().any(|a| a.trim().eq_ignore_ascii_case(t.trim())))
                        .then(|| format!("not one of the allowed values ({})", f.allowed.join(", ")))
                }
                Some((_, p)) if f.ty == "number" && status.filled() && p.value.as_f64().is_none() && leading_number(&p.text()).is_none() => {
                    Some("not a number".into())
                }
                _ => None,
            };
            FieldState {
                key: f.key.clone(),
                required,
                status,
                from_key: found.as_ref().map(|(k, _)| k.clone()).filter(|k| !k.eq_ignore_ascii_case(&f.key)),
                value: found.map(|(_, p)| p),
                reason,
                check,
            }
        })
        .collect()
}

fn leading_number(t: &str) -> Option<f64> {
    let s: String = t.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The form an element of `kind` takes: the project's, else its template's (or the generic).
fn form_of<'a>(st: &'a Store, kind: &str, templates: &[Template], made: &'a mut BTreeMap<String, Form>) -> &'a Form {
    if let Some(f) = st.forms.get(kind) {
        return f;
    }
    made.entry(kind.to_string()).or_insert_with(|| new_form(kind, templates))
}

/// The open conflicts of an element, by key.
fn conflicts_of(st: &Store, handle: &str) -> BTreeSet<String> {
    st.conflicts.iter().filter(|c| c.open && c.handle.eq_ignore_ascii_case(handle)).map(|c| c.key.clone()).collect()
}

/// An element's form with each field's value, status and source (`bim.element`'s `form`).
pub fn element_form(d: &Drawing, e: &Entity) -> Option<Value> {
    let base = super::elemprops::element_kind(e)?;
    let kind = form_kind(e, base.clone());
    let st = stored(d);
    let ts = templates();
    let mut made = BTreeMap::new();
    let form = form_of(&st, &kind, &ts, &mut made).clone();
    let upf = super::elemprops::units_per_foot(d);
    let vals = values(d, e, &base, &kind, upf);
    let states = field_states(&form, &vals, &conflicts_of(&st, &e.handle.hex()));
    let required = states.iter().filter(|s| s.required).count();
    let filled = states.iter().filter(|s| s.required && s.status.filled()).count();
    Some(json!({
        "kind": kind,
        "template": form.template,
        "trade": form.trade,
        "project": st.forms.contains_key(&kind),
        "required": required,
        "filled": filled,
        "missing": states.iter().filter(|s| s.required && matches!(s.status, Status::Missing | Status::Default)).map(|s| s.key.as_str()).collect::<Vec<_>>(),
        "conflicts": states.iter().filter(|s| s.status == Status::Conflict).map(|s| s.key.as_str()).collect::<Vec<_>>(),
        "fields": states.iter().map(state_json).collect::<Vec<_>>(),
    }))
}

fn state_json(s: &FieldState) -> Value {
    let mut v = json!({ "key": s.key, "required": s.required, "status": s.status.as_str() });
    if let Some(p) = &s.value {
        v["value"] = p.value.clone();
        if let Some(u) = &p.unit {
            v["unit"] = json!(u);
        }
        v["source"] = serde_json::to_value(&p.source).unwrap_or(Value::Null);
    }
    if let Some(k) = &s.from_key {
        v["from_key"] = json!(k);
    }
    if let Some(r) = &s.reason {
        v["reason"] = json!(r);
    }
    if let Some(c) = &s.check {
        v["check"] = json!(c);
    }
    v
}

// ─── The project's forms: making and growing them ───────────────────────────

/// Every kind the model holds and the print inventory found an object of.
fn kinds_found(d: &Drawing) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = super::elemprops::elements(d).map(|(e, k)| form_kind(e, k)).filter(|k| !k.is_empty()).collect();
    if let Some(inv) = super::inventory::stored(d) {
        out.extend(inv.marks.iter().filter(|m| m.object && !m.kind.trim().is_empty()).map(|m| norm_kind(&m.kind)).filter(|k| !k.is_empty()));
    }
    out
}

/// Whether a source names a document that defines a kind's attributes (a legend, schedule, spec
/// or submittal: the field is required of every element of the kind) or only one element's (a
/// sheet or a note: the field is optional).
fn defines_kind(k: SourceKind) -> Option<bool> {
    match k {
        SourceKind::Legend | SourceKind::Schedule | SourceKind::Spec | SourceKind::Submittal => Some(true),
        SourceKind::Print | SourceKind::Note => Some(false),
        _ => None,
    }
}

fn source_text(s: &Source) -> String {
    match &s.doc {
        Some(d) => format!("{}: {d}", s.kind.as_str()),
        None => s.kind.as_str().to_string(),
    }
}

/// Add to each project form the fields the documents give its elements that it lacks.
fn grow(st: &mut Store, d: &Drawing, who: &str, only: Option<&BTreeSet<String>>) -> Vec<Value> {
    // Per kind: key → (first document giving it, whether a kind-defining document gives it).
    let mut seen: BTreeMap<String, BTreeMap<String, (Source, bool, Value)>> = BTreeMap::new();
    for (e, base) in super::elemprops::elements(d) {
        let kind = form_kind(e, base);
        if only.is_some_and(|o| !o.contains(&kind)) {
            continue;
        }
        let Some(form) = st.forms.get(&kind) else { continue };
        for (key, p) in props::read(&e.common.xdata).iter() {
            let Some(defines) = defines_kind(p.source.kind) else { continue };
            if !p.has_value() || BOOKKEEPING.iter().any(|b| b.eq_ignore_ascii_case(key)) || form.field(key).is_some() || key.chars().count() > MAX_KEY
            {
                continue;
            }
            let slot =
                seen.entry(kind.clone()).or_default().entry(key.to_ascii_lowercase()).or_insert_with(|| (p.source.clone(), defines, p.value.clone()));
            if defines && !slot.1 {
                *slot = (p.source.clone(), true, p.value.clone());
            }
        }
    }
    let mut out = Vec::new();
    for (kind, keys) in seen {
        for (key, (src, defines, sample)) in keys {
            let Some(form) = st.forms.get_mut(&kind) else { continue };
            if form.fields.len() >= MAX_FIELDS {
                break;
            }
            let field = Field {
                key: key.clone(),
                ty: if sample.is_number() {
                    "number".into()
                } else if sample.is_boolean() {
                    "bool".into()
                } else {
                    "text".into()
                },
                required: defines,
                origin: "added".into(),
                source: Some(cap(&source_text(&src), MAX_TEXT)),
                author: Some(who.to_string()),
                ..Field::default()
            };
            form.fields.push(field.clone());
            st.log(Change {
                who: who.into(),
                why: format!("the {} gives {key} and the form lacked it", src.kind.as_str()),
                source: field.source.clone().unwrap_or_default(),
                kind: kind.clone(),
                key: key.clone(),
                action: "add".into(),
                after: Some(field),
                ..Change::default()
            });
            out.push(json!({"kind": kind, "key": key, "source": source_text(&src), "required": defines}));
        }
    }
    out
}

/// The forms step of the job (after the model build and the print inventory): a project form
/// for every kind the model holds and the inventory found, the fields the documents give added,
/// and the completeness.
pub fn job_pass(s: &mut Session, who: &str) -> Result<Value> {
    let d = s.doc()?;
    let kinds = kinds_found(d);
    let ts = templates();
    let mut st = stored(d);
    let mut created = Vec::new();
    for k in &kinds {
        if !st.forms.contains_key(k) {
            st.ensure(k, &ts, who, "first pass after the model build")?;
            if let Some(f) = st.forms.get(k) {
                created.push(json!({"kind": k, "from": f.template}));
            }
        }
    }
    let grown = grow(&mut st, s.doc()?, who, None);
    save(s, &st)?;
    let c = completeness(s.doc()?, &Query::default());
    let from_template = created.iter().filter(|c| c["from"] != json!(GENERIC)).count();
    Ok(json!({
        "forms": st.forms.len(),
        "created": created.len(),
        "from_template": from_template,
        "generic": created.len() - from_template,
        "fields_added": grown.len(),
        "added": grown.into_iter().take(200).collect::<Vec<_>>(),
        "overall": c["overall"],
        "by_kind": c["kinds"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v["required_fill_pct"].clone())).collect::<serde_json::Map<_, _>>()).unwrap_or_default(),
        "next_work": c["next_work"],
    }))
}

// ─── Completeness ────────────────────────────────────────────────────────────

/// A `bim.completeness` question.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub kind: Option<String>,
    pub field: Option<String>,
    pub level: Option<String>,
    pub room: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Tally {
    required: usize,
    filled: usize,
    documented: usize,
    by_status: BTreeMap<&'static str, usize>,
    by_level: BTreeMap<String, (usize, usize)>,
}

fn pct(n: usize, of: usize) -> Value {
    if of == 0 { Value::Null } else { json!(((n as f64) * 1000.0 / (of as f64)).round() / 10.0) }
}

/// The fill rates of the model's elements.
pub fn completeness(d: &Drawing, q: &Query) -> Value {
    let st = stored(d);
    let ts = templates();
    let upf = super::elemprops::units_per_foot(d);
    let mut made = BTreeMap::new();
    let mut kinds: BTreeMap<String, (usize, String, BTreeMap<String, Tally>)> = BTreeMap::new();
    let want_kind = q.kind.as_deref().map(norm_kind);
    let mut elements = 0usize;
    for (e, base) in super::elemprops::elements(d) {
        let kind = form_kind(e, base.clone());
        if want_kind.as_deref().is_some_and(|w| w != kind) {
            continue;
        }
        let vals = values(d, e, &base, &kind, upf);
        let text = |k: &str| vals.iter().find(|(q, _)| q.eq_ignore_ascii_case(k)).map(|(_, p)| p.text());
        let level = text("level").unwrap_or_default();
        if q.level.as_deref().is_some_and(|l| !l.trim().eq_ignore_ascii_case(level.trim())) {
            continue;
        }
        if q.room.as_deref().is_some_and(|r| !text("room").is_some_and(|x| x.trim().eq_ignore_ascii_case(r.trim()))) {
            continue;
        }
        elements += 1;
        let form = form_of(&st, &kind, &ts, &mut made);
        let template = form.template.clone();
        let states = field_states(form, &vals, &conflicts_of(&st, &e.handle.hex()));
        let slot = kinds.entry(kind).or_insert_with(|| (0, template, BTreeMap::new()));
        slot.0 += 1;
        for s in states {
            if q.field.as_deref().is_some_and(|f| !f.trim().eq_ignore_ascii_case(&s.key)) {
                continue;
            }
            let t = slot.2.entry(s.key.clone()).or_default();
            *t.by_status.entry(s.status.as_str()).or_insert(0) += 1;
            if s.required {
                t.required += 1;
                let lv = t.by_level.entry(level.clone()).or_insert((0, 0));
                lv.0 += 1;
                if s.status.filled() {
                    t.filled += 1;
                    lv.1 += 1;
                }
                if s.status == Status::Documented {
                    t.documented += 1;
                }
            }
        }
    }
    let (mut req, mut fil, mut doc) = (0usize, 0usize, 0usize);
    let mut next: Vec<(f64, usize, String)> = Vec::new();
    let mut out = serde_json::Map::new();
    for (kind, (count, template, fields)) in kinds {
        let project = st.forms.contains_key(&kind);
        let (kr, kf, kd) = fields.values().fold((0, 0, 0), |a, t| (a.0 + t.required, a.1 + t.filled, a.2 + t.documented));
        req += kr;
        fil += kf;
        doc += kd;
        let mut fj = serde_json::Map::new();
        for (key, t) in &fields {
            if t.required > 0 {
                let p = (t.filled as f64) / (t.required as f64);
                next.push((
                    p,
                    t.required,
                    format!(
                        "{kind}.{key} {}% (documented {}%), {} of {} unfilled",
                        pct(t.filled, t.required),
                        pct(t.documented, t.required),
                        t.required - t.filled,
                        t.required
                    ),
                ));
            }
            let mut v = json!({
                "required": t.required,
                "filled": t.filled,
                "documented": t.documented,
                "pct": pct(t.filled, t.required),
                "documented_pct": pct(t.documented, t.required),
                "by_level": t.by_level.iter().map(|(l, (r, f))| (if l.is_empty() { "(none)".to_string() } else { l.clone() }, pct(*f, *r))).collect::<serde_json::Map<_, _>>(),
            });
            for s in ["documented", "derived", "default", "conflict", "missing", "n/a"] {
                v[if s == "n/a" { "na" } else { s }] = json!(t.by_status.get(s).copied().unwrap_or(0));
            }
            // `documented` above counts required fields; the status counts are over all elements.
            v["documented"] = json!(t.documented);
            v["documented_any"] = json!(t.by_status.get("documented").copied().unwrap_or(0));
            fj.insert(key.clone(), v);
        }
        out.insert(
            kind,
            json!({
                "count": count,
                "form": template,
                "project": project,
                "required": kr,
                "filled": kf,
                "required_fill_pct": pct(kf, kr),
                "documented_pct": pct(kd, kr),
                "fields": fj,
            }),
        );
    }
    // The lowest fill first; among equals, the most elements first.
    next.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    json!({
        "elements": elements,
        "overall": {"required": req, "filled": fil, "documented": doc, "pct": pct(fil, req), "documented_pct": pct(doc, req)},
        "kinds": out,
        "next_work": next.into_iter().take(NEXT_WORK).map(|x| x.2).collect::<Vec<_>>(),
        "project_forms": st.forms.len(),
    })
}

/// The merge gate: every kind of `baseline` whose required-field fill is lower now (by more than
/// `tolerance` points).
pub fn gate(now: &Value, baseline: &Value, tolerance: f64) -> Value {
    let mut drops = Vec::new();
    for (kind, b) in baseline["kinds"].as_object().into_iter().flatten() {
        let (Some(before), Some(after)) = (b["required_fill_pct"].as_f64(), now["kinds"][kind]["required_fill_pct"].as_f64()) else { continue };
        if after + tolerance < before {
            drops.push(json!({"kind": kind, "before": before, "after": after}));
        }
    }
    json!({"pass": drops.is_empty(), "drops": drops})
}

// ─── Commands ────────────────────────────────────────────────────────────────

fn author_of(p: &Value) -> String {
    str_param(p, "author").map(|a| cap(a, MAX_KEY)).filter(|a| !a.is_empty()).unwrap_or_else(|| "user".into())
}

fn form_json(f: &Form) -> Value {
    json!({"kind": f.kind, "template": f.template, "trade": f.trade, "label": f.label, "fields": f.fields})
}

fn run_forms(s: &mut Session, p: &Value) -> Result<Value> {
    let ts = templates();
    let kind = str_param(p, "kind").map(norm_kind).filter(|k| !k.is_empty());
    let names: Vec<&str> = ts.iter().map(|t| t.kind.as_str()).collect();
    if bool_or(p, "templates", false) {
        let shown: Vec<Value> = ts
            .iter()
            .filter(|t| kind.as_deref().is_none_or(|k| t.kind == k))
            .map(|t| json!({"kind": t.kind, "template": t.kind, "trade": t.trade, "label": t.label, "aliases": t.aliases, "fields": t.fields}))
            .collect();
        return Ok(json!({"forms": shown, "templates": names, "project": false}));
    }
    let st = stored(s.doc()?);
    if st.forms.is_empty() {
        let mut out = json!({"forms": [], "templates": names, "project": false, "next_step": "bim.form.create makes the project's forms from the templates (the job makes them after the model build)"});
        if let Some(k) = &kind {
            out["forms"] = json!([form_json(&new_form(k, &ts))]);
        }
        return Ok(out);
    }
    let forms: Vec<Value> = st.forms.values().filter(|f| kind.as_deref().is_none_or(|k| f.kind == k)).map(form_json).collect();
    Ok(json!({"forms": forms, "templates": names, "project": true}))
}

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    let who = author_of(p);
    let d = s.doc()?;
    let kinds: BTreeSet<String> = match p.get("kinds") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(norm_kind).filter(|k| !k.is_empty()).take(MAX_FORMS).collect(),
        None | Some(Value::Null) => kinds_found(d),
        Some(_) => return Err(bad(CREATE, "`kinds` is a list of kinds")),
    };
    if kinds.is_empty() {
        return Err(bad(CREATE, "the model holds no elements and the print inventory no objects yet: build the model first, or give `kinds`"));
    }
    let ts = templates();
    let mut st = stored(d);
    let mut created = Vec::new();
    for k in &kinds {
        if !st.forms.contains_key(k) {
            st.ensure(k, &ts, &who, "bim.form.create")?;
            if let Some(f) = st.forms.get(k) {
                created.push(json!({"kind": k, "from": f.template}));
            }
        }
    }
    let grown = if bool_or(p, "grow", true) { grow(&mut st, s.doc()?, &who, Some(&kinds)) } else { Vec::new() };
    save(s, &st)?;
    s.echo(format!("Object forms: {} made, {} fields added from the documents.", created.len(), grown.len()));
    Ok(json!({"created": created, "grown": grown, "forms": st.forms.len()}))
}

fn strings(v: Option<&Value>, n: usize, len: usize) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|t| cap(t, len)).filter(|t| !t.is_empty()).take(n).collect())
}

fn run_field(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind").map(norm_kind).filter(|k| !k.is_empty()).ok_or_else(|| bad(FIELD, "`kind` is required"))?;
    let key = str_param(p, "key").map(|k| cap(k, MAX_KEY)).filter(|k| !k.is_empty()).ok_or_else(|| bad(FIELD, "`key` is required"))?;
    let action = str_param(p, "action")
        .map(|a| a.trim().to_ascii_lowercase())
        .ok_or_else(|| bad(FIELD, "`action` is add, na, applies, require, optional, remove or edit"))?;
    let who = author_of(p);
    let source = str_param(p, "source").map(|t| cap(t, MAX_TEXT)).unwrap_or_default();
    let reason = str_param(p, "reason").map(|t| cap(t, MAX_TEXT)).filter(|t| !t.is_empty());
    let when = match p.get("when") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value::<Cond>(v.clone()).map_err(|e| bad(FIELD, format!("`when`: {e}")))?),
    };
    let ty = str_param(p, "type").map(|t| t.trim().to_ascii_lowercase());
    if ty.as_deref().is_some_and(|t| !["text", "number", "bool", "enum", "ref"].contains(&t)) {
        return Err(bad(FIELD, "`type` is text, number, bool, enum or ref"));
    }
    let ts = templates();
    let mut st = stored(s.doc()?);
    let form = st.ensure(&kind, &ts, &who, "a field of the kind was changed")?;
    let at = form.fields.iter().position(|f| f.key.eq_ignore_ascii_case(&key));
    let before = at.and_then(|i| form.fields.get(i)).cloned();
    let edit = |f: &mut Field| {
        if let Some(t) = &ty {
            f.ty.clone_from(t);
        }
        if let Some(u) = str_param(p, "unit") {
            f.unit = Some(cap(u, MAX_KEY)).filter(|u| !u.is_empty());
        }
        if let Some(a) = strings(p.get("allowed"), 64, MAX_KEY) {
            f.allowed = a;
        }
        if let Some(a) = strings(p.get("aliases"), 16, MAX_KEY) {
            f.aliases = a;
        }
        if let Some(r) = p.get("required").and_then(Value::as_bool) {
            f.required = r;
        }
        if when.is_some() {
            f.when.clone_from(&when);
        }
        if let Some(n) = str_param(p, "note") {
            f.note = Some(cap(n, MAX_TEXT)).filter(|n| !n.is_empty());
        }
    };
    let why = match action.as_str() {
        "add" => {
            if before.is_some() {
                return Err(bad(FIELD, format!("{kind} already has the field {key}: use action edit, require or na")));
            }
            if form.fields.len() >= MAX_FIELDS {
                return Err(bad(FIELD, format!("a form has at most {MAX_FIELDS} fields")));
            }
            let mut f = Field {
                key: key.clone(),
                ty: "text".into(),
                required: true,
                origin: "added".into(),
                source: Some(source.clone()).filter(|t| !t.is_empty()),
                author: Some(who.clone()),
                ..Field::default()
            };
            edit(&mut f);
            form.fields.push(f);
            reason.clone().unwrap_or_else(|| "a document gives it".into())
        }
        "na" => {
            let r = reason.clone().ok_or_else(|| bad(FIELD, "`reason` is required: why the field doesn't apply to this kind"))?;
            let f = at.and_then(|i| form.fields.get_mut(i)).ok_or_else(|| bad(FIELD, format!("{kind} has no field {key}")))?;
            f.na = Some(r.clone());
            r
        }
        "applies" | "require" | "optional" | "edit" => {
            let f = at.and_then(|i| form.fields.get_mut(i)).ok_or_else(|| bad(FIELD, format!("{kind} has no field {key}")))?;
            match action.as_str() {
                "applies" => f.na = None,
                "require" => f.required = true,
                "optional" => f.required = false,
                _ => edit(f),
            }
            reason.clone().unwrap_or_else(|| action.clone())
        }
        "remove" => {
            let i = at.ok_or_else(|| bad(FIELD, format!("{kind} has no field {key}")))?;
            if i < form.fields.len() {
                form.fields.remove(i);
            }
            reason.clone().unwrap_or_else(|| "removed".into())
        }
        _ => return Err(bad(FIELD, "`action` is add, na, applies, require, optional, remove or edit")),
    };
    let after = form.fields.iter().find(|f| f.key.eq_ignore_ascii_case(&key)).cloned();
    let id = st.log(Change { who, why, source, kind: kind.clone(), key: key.clone(), action, before, after: after.clone(), ..Change::default() });
    save(s, &st)?;
    Ok(json!({"kind": kind, "key": key, "change": id, "field": after}))
}

fn run_log(s: &mut Session, p: &Value) -> Result<Value> {
    let st = stored(s.doc()?);
    let kind = str_param(p, "kind").map(norm_kind).filter(|k| !k.is_empty());
    let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |n| usize::try_from(n).unwrap_or(1000).clamp(1, 1000));
    let all: Vec<&Change> = st.log.iter().filter(|c| kind.as_deref().is_none_or(|k| c.kind == k)).collect();
    let shown: Vec<Value> = all
        .iter()
        .rev()
        .take(limit)
        .map(|c| json!({"id": c.id, "at": c.at, "who": c.who, "why": c.why, "source": c.source, "kind": c.kind, "key": c.key, "action": c.action, "reverted": c.reverted, "before": c.before, "after": c.after}))
        .collect();
    Ok(json!({"changes": shown, "total": all.len()}))
}

fn run_revert(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p.get("change").and_then(Value::as_u64).ok_or_else(|| bad(REVERT, "`change` is the id bim.form.log gives"))?;
    let who = author_of(p);
    let mut st = stored(s.doc()?);
    let c = st.log.iter().find(|c| c.id == id).cloned().ok_or_else(|| bad(REVERT, format!("there is no form change {id}")))?;
    if c.reverted {
        return Ok(json!({"reverted": false, "kind": c.kind, "key": c.key, "note": "already reverted"}));
    }
    if c.action == "revert" || c.action == "promote" {
        return Err(bad(REVERT, "a revert or a promotion can't be reverted: change the field again instead"));
    }
    if let Some(later) = st.log.iter().find(|l| {
        l.id > id
            && !l.reverted
            && l.kind == c.kind
            && l.action != "promote"
            && (c.key.is_empty() || l.key.is_empty() || l.key.eq_ignore_ascii_case(&c.key))
            && l.action != "revert"
    }) {
        return Err(bad(REVERT, format!("change {} changed it later: revert that one first", later.id)));
    }
    if c.action == "create" {
        st.forms.remove(&c.kind);
    } else if let Some(form) = st.forms.get_mut(&c.kind) {
        let at = form.fields.iter().position(|f| f.key.eq_ignore_ascii_case(&c.key));
        match (at, &c.before) {
            (Some(i), Some(b)) => {
                if let Some(f) = form.fields.get_mut(i) {
                    *f = b.clone();
                }
            }
            (Some(i), None) => {
                form.fields.remove(i);
            }
            (None, Some(b)) => form.fields.push(b.clone()),
            (None, None) => {}
        }
    }
    if let Some(x) = st.log.iter_mut().find(|x| x.id == id) {
        x.reverted = true;
    }
    st.log(Change {
        who,
        why: format!("reverted change {id}"),
        kind: c.kind.clone(),
        key: c.key.clone(),
        action: "revert".into(),
        before: c.after.clone(),
        after: c.before.clone(),
        ..Change::default()
    });
    save(s, &st)?;
    Ok(json!({"reverted": true, "kind": c.kind, "key": c.key}))
}

fn run_promote(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind").map(norm_kind).filter(|k| !k.is_empty()).ok_or_else(|| bad(PROMOTE, "`kind` is required"))?;
    let mut st = stored(s.doc()?);
    let form = st.forms.get(&kind).cloned().ok_or_else(|| bad(PROMOTE, format!("the project has no form for {kind}")))?;
    let dir = user_folder(str_param(p, "folder")).ok_or_else(|| bad(PROMOTE, "no settings folder: give `folder`"))?;
    let trade = if form.trade.is_empty() { "project".to_string() } else { form.trade.clone() };
    let path = dir.join(format!("{}.json", norm_kind(&trade)));
    let mut file = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<TradeFile>(&t).ok()).unwrap_or_else(|| TradeFile {
        trade: trade.clone(),
        about: "Object form templates promoted from a project (bim.form.promote).".into(),
        templates: Vec::new(),
    });
    let name = if form.template == GENERIC || kind.starts_with("legend:") { kind.clone() } else { form.template.clone() };
    let template = Template {
        kind: name.clone(),
        label: form.label.clone(),
        aliases: if name == kind { Vec::new() } else { vec![kind.clone()] },
        fields: form.fields.iter().map(|f| Field { origin: String::new(), author: None, ..f.clone() }).collect(),
        trade: String::new(),
    };
    file.templates.retain(|t| !t.kind.eq_ignore_ascii_case(&name));
    file.templates.push(template);
    std::fs::create_dir_all(&dir).map_err(|e| bad(PROMOTE, format!("{}: {e}", dir.display())))?;
    let text = serde_json::to_string_pretty(&file).map_err(|e| bad(PROMOTE, e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| bad(PROMOTE, format!("{}: {e}", path.display())))?;
    st.log(Change {
        who: author_of(p),
        why: format!("promoted to the {trade} templates"),
        source: path.display().to_string(),
        kind: kind.clone(),
        action: "promote".into(),
        ..Change::default()
    });
    save(s, &st)?;
    Ok(json!({"path": path.display().to_string(), "kind": name, "fields": form.fields.len()}))
}

fn source_param(p: &Value) -> Result<Source> {
    let v = p
        .get("source")
        .ok_or_else(|| bad(RECORD, "`source` is required: {kind: legend|schedule|spec|submittal|print|note|rule|computed, doc?, at?}"))?;
    let (kind, doc, at) = match v {
        Value::String(k) => (k.as_str(), None, None),
        Value::Object(o) => (
            o.get("kind").and_then(Value::as_str).unwrap_or(""),
            o.get("doc").and_then(Value::as_str),
            o.get("at").and_then(Value::as_array).and_then(|a| match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some([x, y]),
                _ => None,
            }),
        ),
        _ => return Err(bad(RECORD, "`source` is {kind, doc?, at?} or a kind")),
    };
    let k =
        SourceKind::parse(kind).ok_or_else(|| bad(RECORD, "`source.kind` is print, legend, schedule, spec, submittal, note, rule or computed"))?;
    if matches!(k, SourceKind::Default | SourceKind::User) {
        return Err(bad(RECORD, "record a value a document or a rule gives (bim.props.set is a person's edit; a default is not a value)"));
    }
    Ok(Source { kind: k, doc: doc.map(|d| cap(d, MAX_TEXT)).filter(|d| !d.is_empty()), at })
}

/// Whether two values say the same (text without case, or the same number).
fn same_value(a: &Value, b: &Value) -> bool {
    let t = |v: &Value| match v {
        Value::String(s) => s.trim().to_ascii_lowercase(),
        other => other.to_string(),
    };
    let (x, y) = (t(a), t(b));
    x == y || matches!((x.parse::<f64>(), y.parse::<f64>()), (Ok(p), Ok(q)) if (p - q).abs() < 1e-9)
}

fn documented(k: SourceKind) -> bool {
    matches!(
        k,
        SourceKind::Print
            | SourceKind::Legend
            | SourceKind::Schedule
            | SourceKind::Spec
            | SourceKind::Submittal
            | SourceKind::Note
            | SourceKind::User
    )
}

fn conflict_id(handle: &str, key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{handle}|{}", key.to_ascii_lowercase()).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("FC-{:08x}", h & 0xffff_ffff)
}

fn run_record(s: &mut Session, p: &Value) -> Result<Value> {
    let key = str_param(p, "key").map(|k| cap(k, MAX_KEY)).filter(|k| !k.is_empty()).ok_or_else(|| bad(RECORD, "`key` is required"))?;
    let value = match p.get("value") {
        Some(v @ (Value::String(_) | Value::Bool(_))) => v.clone(),
        Some(Value::Number(n)) if n.as_f64().is_some_and(f64::is_finite) => Value::Number(n.clone()),
        _ => return Err(bad(RECORD, "`value` is a string, number or boolean")),
    };
    let source = source_param(p)?;
    let who = author_of(p);
    let unit = str_param(p, "unit").map(|u| cap(u, MAX_KEY)).filter(|u| !u.is_empty());
    let note = str_param(p, "note").map(|n| cap(n, MAX_TEXT)).filter(|n| !n.is_empty());
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(RECORD, "give `handle` or `handles`, or select elements of the model"));
    }
    if hs.len() > MAX_RECORD {
        return Err(bad(RECORD, format!("at most {MAX_RECORD} elements at a time")));
    }
    let ts = templates();
    let d = s.doc()?;
    let upf = super::elemprops::units_per_foot(d);
    let mut st = stored(d);
    let mut writes: Vec<(cadaddy_doc::Handle, String, Prop)> = Vec::new();
    let mut new_conflicts: Vec<Value> = Vec::new();
    let mut added: BTreeSet<String> = BTreeSet::new();
    let mut prop = Prop::new(value.clone(), source.clone());
    prop.unit.clone_from(&unit);
    prop.note.clone_from(&note);
    for h in &hs {
        let d = s.doc()?;
        let e = d.model.get(*h).ok_or_else(|| bad(RECORD, format!("there is no object {} in the model", h.hex())))?;
        let base = super::elemprops::element_kind(e).ok_or_else(|| bad(RECORD, format!("{} is not an element of the model", h.hex())))?;
        let kind = form_kind(e, base.clone());
        let form = st.ensure(&kind, &ts, &who, "a value was recorded for the kind")?;
        let field_key = match form.field(&key) {
            Some(f) => f.key.clone(),
            None => {
                if form.fields.len() >= MAX_FIELDS {
                    return Err(bad(RECORD, format!("{kind}'s form has {MAX_FIELDS} fields, the most it keeps")));
                }
                let f = Field {
                    key: key.clone(),
                    ty: if value.is_number() {
                        "number".into()
                    } else if value.is_boolean() {
                        "bool".into()
                    } else {
                        "text".into()
                    },
                    unit: unit.clone(),
                    required: defines_kind(source.kind).unwrap_or(false),
                    origin: "added".into(),
                    source: Some(source_text(&source)),
                    author: Some(who.clone()),
                    ..Field::default()
                };
                form.fields.push(f.clone());
                st.log(Change {
                    who: who.clone(),
                    why: format!("the {} gives {key} and the form lacked it", source.kind.as_str()),
                    source: source_text(&source),
                    kind: kind.clone(),
                    key: key.clone(),
                    action: "add".into(),
                    after: Some(f),
                    ..Change::default()
                });
                added.insert(kind.clone());
                key.clone()
            }
        };
        // What it says now, under the field's key or an alias.
        let vals = values(d, e, &base, &kind, upf);
        let form = st.forms.get(&kind).cloned().unwrap_or_default();
        let field = form.field(&field_key).cloned().unwrap_or_default();
        let now = vals.iter().find(|(k, _)| field.names(k)).map(|(k, p)| (k.clone(), p.clone()));
        let hex = h.hex();
        match now {
            Some((_, old))
                if documented(old.source.kind)
                    && !same_value(&old.value, &value)
                    && (old.source.kind != source.kind || old.source.doc != source.doc) =>
            {
                let id = conflict_id(&hex, &field_key);
                let given = Given { value: value.clone(), source: source.clone(), author: who.clone() };
                let at = st.conflicts.iter().position(|c| c.id == id);
                if let Some(c) = at.and_then(|i| st.conflicts.get_mut(i)) {
                    if !c.values.iter().any(|g| same_value(&g.value, &value) && g.source == source) {
                        c.values.push(given);
                        c.values.truncate(16);
                    }
                    c.open = true;
                    c.answer = None;
                } else if st.conflicts.len() < MAX_CONFLICTS {
                    st.conflicts.push(Conflict {
                        id: id.clone(),
                        handle: hex.clone(),
                        kind: kind.clone(),
                        key: field_key.clone(),
                        values: vec![Given { value: old.value.clone(), source: old.source.clone(), author: String::new() }, given],
                        open: true,
                        answer: None,
                    });
                }
                new_conflicts.push(json!({"id": id, "handle": hex, "key": field_key, "values": [{"value": old.value, "source": old.source}, {"value": value, "source": source}]}));
            }
            _ => {
                let mut pr = prop.clone();
                pr.unit = pr.unit.or_else(|| field.unit.clone());
                writes.push((*h, field_key.clone(), pr));
            }
        }
    }
    // Write: generated elements as overrides (a rebuild keeps them), the others into their store.
    let d = s.doc()?;
    let entries: Vec<(cadaddy_doc::Handle, Entry)> = writes.into_iter().map(|(h, key, prop)| (h, Entry { key, prop })).collect();
    let (generated, own): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|(h, _)| d.model.get(*h).is_some_and(|e| super::revise3d::tag_key(e).is_some()));
    let batch: Vec<(cadaddy_doc::Handle, Vec<Entry>)> = generated.iter().map(|(h, en)| (*h, vec![en.clone()])).collect();
    let labels = if batch.is_empty() { Vec::new() } else { super::overrides::set_props(s, RECORD, &batch, cadaddy_doc::overrides::Author::Agent)? };
    let d = s.doc_mut()?;
    let mut written = labels.len();
    for (h, en) in &own {
        let ok = d
            .modify_entity(*h, |e| {
                let mut store: Props = props::read(&e.common.xdata);
                store.set(&en.key, en.prop.clone());
                e.common.xdata = props::write(&e.common.xdata, &store).into();
            })
            .is_ok();
        if ok {
            written += 1;
        }
    }
    save(s, &st)?;
    Ok(json!({
        "set": written,
        "conflicts": new_conflicts,
        "added_fields": added.iter().map(|k| json!({"kind": k, "key": key})).collect::<Vec<_>>(),
    }))
}

fn run_completeness(s: &mut Session, p: &Value) -> Result<Value> {
    let q = Query {
        kind: str_param(p, "kind").map(str::to_string).filter(|k| !k.trim().is_empty()),
        field: str_param(p, "field").map(str::to_string).filter(|k| !k.trim().is_empty()),
        level: str_param(p, "level").map(str::to_string).filter(|k| !k.trim().is_empty()),
        room: str_param(p, "room").map(str::to_string).filter(|k| !k.trim().is_empty()),
    };
    let mut r = completeness(s.doc()?, &q);
    if r["elements"] == json!(0) {
        r["next_step"] = json!("no elements match: build the model (model.build) or loosen the filters");
    }
    match p.get("baseline") {
        None | Some(Value::Null) => {}
        Some(b @ Value::Object(_)) => {
            let tol = p.get("tolerance_pct").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.0);
            r["gate"] = gate(&r, b, tol);
        }
        Some(_) => return Err(bad(COMPLETENESS, "`baseline` is an earlier bim.completeness answer")),
    }
    Ok(r)
}

// ─── Conflicts as review items ───────────────────────────────────────────────

/// The open conflicts (each a `conflict` review item).
pub fn open_conflicts(d: &Drawing) -> Vec<Conflict> {
    stored(d).conflicts.into_iter().filter(|c| c.open).collect()
}

/// Settle conflict `id`: the field takes `answer` (a person's or agent's decision, citing the
/// document that settles it), and the conflict closes.
pub fn resolve_conflict(s: &mut Session, id: &str, answer: &Value, cite: &str, who: &str) -> Result<()> {
    const CMD: &str = "review.resolve";
    let value = match answer {
        Value::String(t) if !t.trim().is_empty() => json!(cap(t, MAX_TEXT)),
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => answer.clone(),
        Value::Bool(_) => answer.clone(),
        _ => return Err(bad(CMD, "a conflict's answer is the value that holds (text, number or yes/no), citing the document that settles it")),
    };
    let mut st = stored(s.doc()?);
    let c = st.conflicts.iter().find(|c| c.id == id).cloned().ok_or_else(|| bad(CMD, format!("there is no conflict {id}")))?;
    let h = cadaddy_doc::Handle(u64::from_str_radix(&c.handle, 16).map_err(|_| bad(CMD, "the conflict's element is gone"))?);
    let mut prop = Prop::new(value.clone(), Source::user());
    prop.note = Some(cap(&format!("settles conflict {id} ({who}){}", if cite.is_empty() { String::new() } else { format!(": {cite}") }), MAX_TEXT));
    let entry = Entry { key: c.key.clone(), prop };
    let d = s.doc()?;
    let generated = d.model.get(h).is_some_and(|e| super::revise3d::tag_key(e).is_some());
    if generated {
        super::overrides::set_props(s, CMD, &[(h, vec![entry])], cadaddy_doc::overrides::Author::User)?;
    } else {
        s.doc_mut()?
            .modify_entity(h, |e| {
                let mut store: Props = props::read(&e.common.xdata);
                store.set(&entry.key, entry.prop.clone());
                e.common.xdata = props::write(&e.common.xdata, &store).into();
            })
            .map_err(|_| bad(CMD, "the conflict's element is gone"))?;
    }
    if let Some(x) = st.conflicts.iter_mut().find(|x| x.id == id) {
        x.open = false;
        x.answer = Some(value);
    }
    save(s, &st)
}

#[cfg(test)]
#[path = "forms_tests.rs"]
mod tests;
