//! What an element with a generic kind is, for Validate's rules (Q6128). The PDF→BIM job builds
//! its devices as kind `device` (a print symbol's block, `PDF-SYM-12`, or the build's
//! `3D-Devices`) with the plan tag as their mark, so a rule for receptacles or switches would
//! match none of them. Such an element is read as what these say, first that answers:
//!
//! 1. the element itself: the kind the job stored on it (its `legend_kind`, `kind` or
//!    `device_kind` property, the build's `kind:` tag, a terminal's or a piece of equipment's
//!    own kind, a built part named for what it is: `diffuser`);
//! 2. its BIM type: the category of the type its mark names;
//! 3. the job's legend and symbol table: the building spec's tag for the device (the build's
//!    `devid:`), else what the legends on the job's sheets say its mark is (a device legend's or
//!    schedule's row, the mark in its first column and the words after it; a `KEY=MEANING`
//!    designation), else the kind the spec's tags, the prints' symbols and the schedules give
//!    its mark (a tag's legend, device type or description words naming a kind the model
//!    holds);
//! 4. the project rule `validate.kind.<mark>` (domain `custom`): `validate.kind.REC =
//!    receptacle`.
//!
//! There is no built-in list of marks: a mark means what this job's documents or this
//! project's rules say it means. An element none of these name keeps its kind and is listed
//! as unmatched by the prescreen.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::projrules::{ProjectRules, RuleDomain};
use cadaddy_doc::{Drawing, Entity};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::Prints;
use cadaddy_walk3d::validate_kinds;

/// The project-rule key prefix of a mark's kind.
pub const KIND_PREFIX: &str = "validate.kind.";
/// Words that say only that an element is something: a kind made of these (and numbers, and
/// single letters) is generic.
const GENERIC_WORDS: [&str; 17] = [
    "device",
    "devices",
    "fixture",
    "fixtures",
    "equipment",
    "terminal",
    "terminals",
    "symbol",
    "sym",
    "pdf",
    "item",
    "element",
    "object",
    "block",
    "generic",
    "unknown",
    "other",
];
/// Spec tags, symbols and schedule rows read, at most.
const MAX_ROWS: usize = 500_000;
/// Objects looked at for the legends' texts, at most (every one).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Legend headings, and texts kept round them, at most.
const MAX_HEADINGS: usize = 200;
const MAX_LEGEND_TEXTS: usize = 100_000;
/// A legend's rows reach at most this many heading heights below it, from this many left of
/// it to this many right; a blank band this many text heights tall ends it.
const LEGEND_DEPTH: f64 = 220.0;
const LEGEND_LEFT: f64 = 12.0;
const LEGEND_RIGHT: f64 = 140.0;
const LEGEND_GAP: f64 = 7.0;
/// Longest mark and kind read.
const MAX_NAME: usize = 60;
/// Longest mark read from a legend's first column.
const MAX_MARK: usize = 8;

/// Lower case, words joined by `_`.
fn words(s: &str) -> String {
    let t: String = s.trim().chars().take(200).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    t.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

/// Does `kind` say nothing about what the element is (`device`, `PDF-SYM-12`, `3D-Devices`)?
pub fn is_generic(kind: &str) -> bool {
    !words(kind).split('_').any(|w| w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic()) && !GENERIC_WORDS.contains(&w))
}

/// The kind the model holds that `text` names: the longest one whose words it holds in a row
/// (`DUPLEX RECEPTACLE` → `receptacle`, `EXHAUST FAN` → `exhaust_fan`, `return_grille` →
/// `grille`); generic kinds (`fixture`, `equipment`) are not looked for.
pub fn model_kind_in(text: &str) -> Option<&'static str> {
    let w = format!("_{}_", words(text));
    validate_kinds::MODEL_KINDS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !is_generic(k) && w.contains(&format!("_{k}_")))
        .max_by_key(|k| (k.len(), std::cmp::Reverse(*k)))
}

/// A mark as compared: trimmed, upper case; none when empty or too long.
fn mark_key(m: &str) -> Option<String> {
    let m = m.trim();
    (!m.is_empty() && m.len() <= MAX_NAME).then(|| m.to_ascii_uppercase())
}

/// Is `key` a mark's kind (`validate.kind.<mark>`)?
pub fn is_kind_key(key: &str) -> bool {
    key.trim().get(..KIND_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(KIND_PREFIX))
}

/// Read a project rule `validate.kind.<mark> = <kind>`: (the mark, upper case; the kind, as
/// words). `project.rules.set` refuses one that doesn't read.
pub fn check_kind_rule(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<(String, String), String> {
    if domain != RuleDomain::Custom {
        return Err(format!("`{key}` names what a mark is: set it in domain `custom`"));
    }
    let mark = key
        .trim()
        .get(KIND_PREFIX.len()..)
        .and_then(mark_key)
        .ok_or_else(|| format!("`{key}`: name the mark after `{KIND_PREFIX}` (e.g. `{KIND_PREFIX}REC`), at most {MAX_NAME} characters"))?;
    let kind = words(value);
    if kind.is_empty() || kind.len() > MAX_NAME {
        return Err(format!("`{key}`: give the kind the mark is (e.g. receptacle, switch, smoke_detector), at most {MAX_NAME} characters"));
    }
    if is_generic(&kind) {
        return Err(format!("`{key}`: `{value}` says only that it is something; give the kind (e.g. receptacle, switch, exhaust_fan)"));
    }
    Ok((mark, kind))
}

/// An element's kind as Validate reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub kind: String,
    /// Where the kind came from: `element` (its own kind), `stored` (the kind the job stored on
    /// it), `bim_type` (its type's category), `legend` (a legend on the job's sheets),
    /// `symbol_table` (the job's tags, symbols and schedules), `project_rule`
    /// (`validate.kind.<mark>`), `unresolved` (generic, nothing names it).
    pub from: &'static str,
    /// Its BIM category: the IFC class it is written as (`IfcOutlet`, `IfcAirTerminal`).
    pub category: Option<String>,
}

impl Resolved {
    /// Whether nothing said what the generic element is.
    pub fn unresolved(&self) -> bool {
        self.from == "unresolved"
    }
}

/// The kind the element stores, when it says what it is.
fn stored(e: &Entity) -> Option<String> {
    let props = cadaddy_doc::props::read(&e.common.xdata);
    for key in ["legend_kind", "kind", "device_kind"] {
        if let Some(p) = props.get(key).filter(|p| p.has_value()) {
            let t = p.text();
            if !is_generic(&t) {
                return Some(words(&t));
            }
        }
    }
    let part = super::wallgen::made_as(e)?.to_ascii_lowercase();
    let own = super::bimlook::made_tag(e, "part").and_then(|p| p.strip_prefix("terminal:").or_else(|| p.strip_prefix("equipment:")));
    if let Some(k) = own.filter(|k| !is_generic(k)) {
        return Some(words(k));
    }
    if let Some(k) = super::bimlook::made_tag(e, "kind").filter(|k| !is_generic(k)) {
        return Some(words(k));
    }
    (part != "mep" && !is_generic(&part)).then(|| words(&part))
}

/// An element's mark: its build tag, its `type_mark` or `mark` property, else the BIM type it
/// is assigned.
pub fn mark_of(e: &Entity) -> Option<String> {
    let tag = super::bimlook::made_tag(e, "tag").filter(|t| !t.trim().is_empty()).map(|t| t.trim().to_string());
    tag.or_else(|| {
        let props = cadaddy_doc::props::read(&e.common.xdata);
        ["type_mark", "mark"].iter().find_map(|k| props.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()))
    })
    .or_else(|| cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata).map(|m| m.trim().to_string()))
    .filter(|m| !m.is_empty() && m.len() <= MAX_NAME)
}

/// The IFC class an element is written as, else the one its kind is.
fn category(e: Option<&Entity>, kind: &str) -> Option<String> {
    e.map(|e| cadaddy_walk3d::fourd::element_of(e).class)
        .filter(|c| c != "IfcBuildingElementProxy")
        .or_else(|| validate_kinds::category_of(kind))
        .or_else(|| model_kind_in(kind).and_then(validate_kinds::category_of))
}

/// What one of the job's tables says of a mark, in its own words (a legend row, a schedule row,
/// a spec device tag): for the facts a kind doesn't hold (tamper-resistance, Q6142).
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    /// `legend`, `schedule` or `spec_tag` (a device's own).
    pub from: &'static str,
    /// Where: `legend "<heading>"`, `schedule <name> on <sheet>`, `spec tag <id>`.
    pub source: String,
    pub text: String,
    /// A legend row's heading (`LIGHTING FIXTURE SCHEDULE`), when it has one.
    pub heading: Option<String>,
}

/// Texts kept per mark, at most, and their longest.
const MAX_SAID: usize = 4;
const MAX_SAID_TEXT: usize = 300;

/// What the job's tables, the BIM types and the project say each mark (and each spec device)
/// is.
#[derive(Clone, Debug, Default)]
pub struct Kinds {
    /// The spec's device tags: id → kind.
    by_devid: HashMap<String, String>,
    /// Mark → what the legends on the job's sheets say it is.
    legend: HashMap<String, String>,
    /// Mark → the kind most of the job's tags, symbols and schedule rows with it give.
    by_mark: HashMap<String, String>,
    /// Mark → its BIM type's category.
    types: HashMap<String, String>,
    /// Mark → `validate.kind.<mark>`.
    project: HashMap<String, String>,
    /// Mark → what the job's legend rows and schedule rows say of it, in words.
    said: HashMap<String, Vec<Said>>,
    /// The spec's device tags: id → what it says of the device, in words (the documents' fields
    /// only: none the job inferred from a rule or a default).
    said_devid: HashMap<String, Said>,
    /// The spec's device tags: id → what the job inferred of it from a rule (`source: rule`), in
    /// words: the job's own assumption, no document's.
    assumed_devid: HashMap<String, String>,
}

/// A tag field's source that is the job's own inference, not a document's.
fn inferred(source: &str) -> bool {
    matches!(source.trim().to_ascii_lowercase().as_str(), "rule" | "default")
}

fn keep_said(said: &mut HashMap<String, Vec<Said>>, mark: Option<String>, from: &'static str, source: String, text: String, heading: Option<String>) {
    let (Some(m), text) = (mark, text.trim().chars().take(MAX_SAID_TEXT).collect::<String>()) else { return };
    if text.is_empty() {
        return;
    }
    let list = said.entry(m).or_default();
    if list.len() < MAX_SAID && !list.iter().any(|x| x.text == text) {
        list.push(Said { from, source, text, heading });
    }
}

impl Kinds {
    /// Read the tables of a drawing, its building spec and the prints' facts.
    pub fn of(d: &Drawing, sp: &BuildingSpec, prints: Option<&Prints>) -> Kinds {
        let mut votes: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
        let mut vote = |mark: Option<String>, kind: String| {
            if let Some(m) = mark {
                *votes.entry(m).or_default().entry(kind).or_default() += 1;
            }
        };
        // A kind said in words: the kind itself, else a legend's, type's or description's words.
        let said = |kind: &str, texts: &[&str]| -> Option<String> {
            if !is_generic(kind) {
                return Some(words(kind));
            }
            texts.iter().find_map(|t| model_kind_in(t)).map(str::to_string)
        };
        let mut by_devid = HashMap::new();
        let mut texts: HashMap<String, Vec<Said>> = HashMap::new();
        let mut said_devid = HashMap::new();
        let mut assumed_devid = HashMap::new();
        let prints = prints.or(sp.prints.as_deref());
        for t in sp.tags.iter().take(MAX_ROWS).filter(|t| t.id.starts_with("device:")) {
            let field = |k: &str| t.fields.get(k).map(|f| f.value.as_str());
            let words_of: Vec<&str> = ["legend", "device_type", "description", "fixture"].iter().filter_map(|k| field(k)).collect();
            let mark = ["type", "type_mark", "mark"].iter().find_map(|k| field(k).and_then(mark_key));
            let documented: Vec<&str> = ["legend", "device_type", "description", "fixture"]
                .iter()
                .filter_map(|k| t.fields.get(*k).filter(|f| !inferred(&f.source)).map(|f| f.value.as_str()))
                .collect();
            let assumed: Vec<String> =
                t.fields.iter().filter(|(_, f)| inferred(&f.source)).take(20).map(|(k, f)| format!("{k}: {}", f.value)).collect();
            if !assumed.is_empty() && assumed_devid.len() < MAX_ROWS {
                assumed_devid.insert(t.id.clone(), assumed.join("; ").chars().take(MAX_SAID_TEXT).collect::<String>());
            }
            let text = documented.join("; ");
            if !text.trim().is_empty() && said_devid.len() < MAX_ROWS {
                let source = format!("spec tag {}", t.id.chars().take(80).collect::<String>());
                // One device's own words: never every device of its mark.
                said_devid.insert(t.id.clone(), Said { from: "spec_tag", source, text: text.chars().take(MAX_SAID_TEXT).collect(), heading: None });
            }
            let Some(kind) = said(&t.kind, &words_of) else {
                continue;
            };
            by_devid.insert(t.id.clone(), kind.clone());
            vote(mark, kind);
        }
        for s in prints.map_or(&[][..], |p| p.symbols.as_slice()).iter().take(MAX_ROWS) {
            if let Some(kind) = said(&s.kind, &[]) {
                vote(s.mark.as_deref().and_then(mark_key), kind);
            }
        }
        for r in prints.map_or(&[][..], |p| p.schedule.as_slice()).iter().take(MAX_ROWS) {
            let row: Vec<&str> = r.fields.values().take(40).map(String::as_str).collect();
            let source = format!("schedule {} on {}, mark {}", r.schedule, r.sheet, r.mark).chars().take(160).collect();
            keep_said(&mut texts, mark_key(&r.mark), "schedule", source, row.join("; "), None);
            let kind = r.kind.as_deref().unwrap_or_default();
            if let Some(kind) =
                said(kind, &["description", "type", "item"].iter().filter_map(|k| r.fields.get(*k).map(String::as_str)).collect::<Vec<_>>())
            {
                vote(mark_key(&r.mark), kind);
            }
        }
        let by_mark = votes
            .into_iter()
            .filter_map(|(m, ks)| ks.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(k, _)| (m, k)))
            .collect();
        let types = d
            .bim_types
            .types
            .iter()
            .take(MAX_ROWS)
            .filter(|t| !is_generic(&t.category))
            .filter_map(|t| Some((mark_key(&t.mark)?, words(&t.category))))
            .collect();
        let mut legend = HashMap::new();
        for (mark, row, heading) in legend_rows(d) {
            if let Some(kind) = model_kind_in(&row) {
                legend.entry(mark.clone()).or_insert_with(|| kind.to_string());
            }
            let source = heading.as_deref().map_or_else(|| "legend".to_string(), |h| format!("legend \"{h}\""));
            keep_said(&mut texts, Some(mark), "legend", source, row, heading);
        }
        Kinds { by_devid, legend, by_mark, types, project: project_kinds(&d.project_rules), said: texts, said_devid, assumed_devid }
    }

    /// What the job inferred of an element from a rule or a default, not from a document (its
    /// spec device tag's fields of source `rule`/`default`), in words.
    pub fn assumed_of(&self, e: Option<&Entity>) -> Option<&str> {
        e.and_then(|e| super::bimlook::made_tag(e, "devid")).and_then(|id| self.assumed_devid.get(id)).map(String::as_str)
    }

    /// What the job's tables say of an element in words: its spec device tag's (the build's
    /// `devid:`), then its mark's legend rows and schedule rows.
    pub fn said_of(&self, mark: Option<&str>, e: Option<&Entity>) -> Vec<&Said> {
        let own = e.and_then(|e| super::bimlook::made_tag(e, "devid")).and_then(|id| self.said_devid.get(id));
        own.into_iter().chain(mark.and_then(mark_key).and_then(|m| self.said.get(&m)).into_iter().flatten()).collect()
    }

    /// Every mark's legend rows (each with its heading), for the tables a legend heading names
    /// (a drawn lighting fixture schedule).
    pub fn legend_said(&self) -> impl Iterator<Item = (&String, &Said)> {
        self.said.iter().flat_map(|(m, list)| list.iter().filter(|s| s.from == "legend").map(move |s| (m, s)))
    }

    /// What an element of `kind` with `mark` (and its object `e`, when there is one) is.
    ///
    /// A connection to equipment (`equipment_connection`) says only that it connects something:
    /// it is what the job's tables or the project say its mark is (an `EF` row naming an
    /// exhaust fan makes it an exhaust fan), else it stays a connection.
    pub fn resolve(&self, kind: &str, mark: Option<&str>, e: Option<&Entity>) -> Resolved {
        let done = |kind: String, from: &'static str| Resolved { category: category(e, &kind), kind, from };
        if !is_generic(kind) {
            if is_connection(kind)
                && let Some((k, from)) = self.by_tables(mark, e).filter(|(k, _)| !is_connection(k))
            {
                return done(k, from);
            }
            return done(kind.to_string(), "element");
        }
        if let Some(k) = e.and_then(stored) {
            return done(k, "stored");
        }
        match self.by_tables(mark, e) {
            Some((k, from)) => done(k, from),
            None => done(kind.to_string(), "unresolved"),
        }
    }

    /// What the job's tables and the project say an element with `mark` is: its BIM type, the
    /// spec's device tag, a legend row, the symbols and schedules, `validate.kind.<mark>`.
    fn by_tables(&self, mark: Option<&str>, e: Option<&Entity>) -> Option<(String, &'static str)> {
        let mark = mark.and_then(mark_key);
        let by = |m: &HashMap<String, String>| mark.as_ref().and_then(|k| m.get(k)).cloned();
        if let Some(k) = by(&self.types) {
            return Some((k, "bim_type"));
        }
        if let Some(k) = e.and_then(|e| super::bimlook::made_tag(e, "devid")).and_then(|id| self.by_devid.get(id)).cloned() {
            return Some((k, "symbol_table"));
        }
        if let Some(k) = by(&self.legend) {
            return Some((k, "legend"));
        }
        if let Some(k) = by(&self.by_mark) {
            return Some((k, "symbol_table"));
        }
        by(&self.project).map(|k| (k, "project_rule"))
    }
}

/// Is `kind` a connection to equipment (`equipment_connection`): what it is, its mark says?
fn is_connection(kind: &str) -> bool {
    let w = words(kind);
    w == "connection" || w.ends_with("_connection")
}

/// Is `t` shaped like a mark in a legend's first column: one short word of capitals and digits
/// (dashes and dots between), a letter in it (`REC`, `GFCI`, `S3`, `EF-1`)?
fn looks_like_mark(t: &str) -> bool {
    let t = t.trim();
    (1..=MAX_MARK).contains(&t.len())
        && t.chars().any(|c| c.is_ascii_uppercase())
        && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && !t.starts_with(['-', '.'])
        && !t.ends_with(['-', '.'])
}

/// What the legends in the drawing say of each mark: each device legend's or schedule's rows
/// (the mark first, the words after it; with the heading they stand under), then the
/// `KEY=MEANING` designations, in that order.
fn legend_rows(d: &Drawing) -> Vec<(String, String, Option<String>)> {
    use cadaddy_mepfix::device_types::{designations, is_device_table_heading};
    use cadaddy_mepfix::sheet_facts::TextItem;
    let text = |e: &Entity| -> Option<TextItem> {
        let (value, at, height) = match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height),
            cadaddy_doc::EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height),
            _ => return None,
        };
        let value = value.trim();
        (!value.is_empty() && value.len() <= 200 && at.is_finite() && height.is_finite() && height > 0.0).then(|| TextItem {
            value: value.to_string(),
            at,
            height,
            rotation: 0.0,
        })
    };
    let headings: Vec<TextItem> =
        d.model.iter().take(MAX_ENTITIES).filter_map(|e| text(e)).filter(|t| is_device_table_heading(&t.value)).take(MAX_HEADINGS).collect();
    let reach = |h: &TextItem, p: cadaddy_geom::Vec2| {
        p.x >= h.at.x - LEGEND_LEFT * h.height
            && p.x <= h.at.x + LEGEND_RIGHT * h.height
            && p.y < h.at.y - 0.2 * h.height
            && p.y > h.at.y - LEGEND_DEPTH * h.height
    };
    let mut keyed = Vec::new();
    let mut near = Vec::new();
    for t in d.model.iter().take(MAX_ENTITIES).filter_map(|e| text(e)) {
        if keyed.len() + near.len() >= MAX_LEGEND_TEXTS {
            break;
        }
        if t.value.contains('=') {
            keyed.push(t);
        } else if headings.iter().any(|h| reach(h, t.at)) {
            near.push(t);
        }
    }
    let mut out: Vec<(String, String, Option<String>)> = Vec::new();
    for h in &headings {
        let mut below: Vec<&TextItem> = near.iter().filter(|t| reach(h, t.at)).collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Rows, top down, until a blank band.
        let mut rows: Vec<Vec<&TextItem>> = Vec::new();
        let (mut last_y, mut last_h) = (h.at.y, h.height);
        for t in below {
            let gap = last_y - t.at.y;
            if gap > LEGEND_GAP * last_h.max(t.height) {
                break;
            }
            match rows.last_mut() {
                Some(row) if gap <= 0.5 * t.height.min(last_h) => row.push(t),
                _ => rows.push(vec![t]),
            }
            (last_y, last_h) = (t.at.y, t.height);
        }
        for mut row in rows {
            row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            let Some(i) = row.iter().position(|t| looks_like_mark(&t.value)) else { continue };
            let said: Vec<&str> = row.iter().skip(i + 1).map(|t| t.value.as_str()).collect();
            if let Some(mark) = row.get(i).and_then(|t| mark_key(&t.value))
                && !said.is_empty()
            {
                out.push((mark, said.join(" "), Some(h.value.chars().take(MAX_NAME).collect())));
            }
        }
    }
    for (k, meaning) in designations(&keyed) {
        if let Some(mark) = mark_key(&k) {
            out.push((mark, meaning, None));
        }
    }
    out
}

/// The project's `validate.kind.<mark>` rules that read: mark → kind.
fn project_kinds(pr: &ProjectRules) -> HashMap<String, String> {
    pr.list_domain(RuleDomain::Custom)
        .into_iter()
        .filter(|e| is_kind_key(&e.key))
        .filter_map(|e| check_kind_rule(RuleDomain::Custom, &e.key, &e.active.value).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::tags::ElementTag;

    #[test]
    fn generic_kinds_say_nothing_and_named_kinds_do() {
        for k in ["device", "PDF-SYM-12", "3D-Devices", "", "fixture", "equipment", "E", "terminal 4"] {
            assert!(is_generic(k), "{k}");
        }
        for k in ["receptacle", "gfci_receptacle", "return_grille", "occupancy_sensor", "light_fixture", "plumbing_fixture"] {
            assert!(!is_generic(k), "{k}");
        }
    }

    #[test]
    fn words_name_the_longest_kind_the_model_holds() {
        assert_eq!(model_kind_in("DUPLEX RECEPTACLE"), Some("receptacle"));
        assert_eq!(model_kind_in("EXHAUST FAN"), Some("exhaust_fan"));
        assert_eq!(model_kind_in("return_grille"), Some("grille"));
        assert_eq!(model_kind_in("CEILING SMOKE DETECTOR"), Some("smoke_detector"));
        assert_eq!(model_kind_in("device type A"), None);
        assert_eq!(model_kind_in("PLUMBING FIXTURE"), None);
    }

    #[test]
    fn a_marks_kind_rule_reads_or_is_refused() {
        assert_eq!(check_kind_rule(RuleDomain::Custom, "validate.kind.rec", " Receptacle "), Ok(("REC".into(), "receptacle".into())));
        assert_eq!(check_kind_rule(RuleDomain::Custom, "VALIDATE.KIND.EF", "exhaust fan"), Ok(("EF".into(), "exhaust_fan".into())));
        assert!(check_kind_rule(RuleDomain::Tolerance, "validate.kind.REC", "receptacle").unwrap_err().contains("custom"));
        assert!(check_kind_rule(RuleDomain::Custom, "validate.kind.", "receptacle").unwrap_err().contains("name the mark"));
        assert!(check_kind_rule(RuleDomain::Custom, "validate.kind.REC", "").is_err());
        assert!(check_kind_rule(RuleDomain::Custom, "validate.kind.REC", "device").unwrap_err().contains("says only"));
        assert!(is_kind_key("validate.kind.S3") && !is_kind_key("validate.G-HOST.severity"));
    }

    fn text(d: &mut Drawing, value: &str, x: f64, y: f64) {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: 1.0,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::default(),
            valign: cadaddy_doc::VAlign::default(),
        };
        let _ = d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t));
    }

    #[test]
    fn a_legends_rows_and_designations_say_what_each_mark_is() {
        let mut d = Drawing::new_imperial();
        text(&mut d, "ELECTRICAL SYMBOL LEGEND", 100.0, 100.0);
        for (i, (mark, said)) in
            [("REC", "DUPLEX RECEPTACLE, 18\" AFF"), ("GFCI", "GFCI DUPLEX RECEPTACLE"), ("SD", "SMOKE DETECTOR"), ("A", "TYPE A")].iter().enumerate()
        {
            let y = 97.0 - 2.0 * i as f64;
            text(&mut d, mark, 98.0, y);
            text(&mut d, said, 104.0, y);
        }
        // Far below the legend, a mark beside other words is no legend row.
        text(&mut d, "F", 98.0, 20.0);
        text(&mut d, "EXHAUST FAN BY OTHERS", 104.0, 20.0);
        text(&mut d, "TV=TELEVISION OUTLET", 300.0, 300.0);
        let k = Kinds::of(&d, &BuildingSpec::default(), None);
        let of = |m: &str| k.resolve("device", Some(m), None);
        assert_eq!((of("REC").kind.as_str(), of("REC").from), ("receptacle", "legend"));
        assert_eq!(of("gfci").kind, "receptacle");
        assert_eq!(of("SD").kind, "smoke_detector");
        assert_eq!(of("TV").kind, "outlet");
        assert!(of("A").unresolved() && of("F").unresolved(), "{:?} {:?}", of("A"), of("F"));
    }

    fn tag(id: &str, kind: &str, fields: &[(&str, &str)]) -> ElementTag {
        let mut t = ElementTag { id: id.into(), kind: kind.into(), level: "Level 1".into(), ..ElementTag::default() };
        for (k, v) in fields {
            t.set(k, *v, "plan");
        }
        t
    }

    #[test]
    fn the_jobs_tables_then_the_project_name_a_mark() {
        let mut d = Drawing::new_imperial();
        let sp = BuildingSpec {
            tags: vec![
                tag("device:Level 1:1,1", "receptacle", &[("type", "REC")]),
                tag("device:Level 1:2,1", "receptacle", &[("type", "REC")]),
                tag("device:Level 1:3,1", "device", &[("type", "REC")]),
                tag("device:Level 1:4,1", "device", &[("type", "SD"), ("legend", "SMOKE DETECTOR, CEILING MOUNTED")]),
                tag("device:Level 1:5,1", "device", &[("type", "A"), ("device_type", "device type A")]),
            ],
            ..BuildingSpec::default()
        };
        let p = cadaddy_doc::projrules::RuleValue::new("exhaust_fan");
        d.project_rules.set(RuleDomain::Custom, "validate.kind.EF", p, "the fan's tag");
        d.project_rules.set(RuleDomain::Custom, "validate.kind.REC", cadaddy_doc::projrules::RuleValue::new("switch"), "a wrong guess");
        let k = Kinds::of(&d, &sp, None);
        let r = k.resolve("device", Some("rec"), None);
        assert_eq!((r.kind.as_str(), r.from, r.category.as_deref()), ("receptacle", "symbol_table", Some("IfcOutlet")), "the job's tables first");
        assert_eq!(k.resolve("PDF-SYM-4", Some("SD"), None).kind, "smoke_detector");
        let ef = k.resolve("device", Some("EF"), None);
        assert_eq!((ef.kind.as_str(), ef.from, ef.category.as_deref()), ("exhaust_fan", "project_rule", Some("IfcFan")));
        let a = k.resolve("device", Some("A"), None);
        assert!(a.unresolved() && a.kind == "device", "{a:?}");
        let own = k.resolve("switch", Some("REC"), None);
        assert_eq!((own.kind.as_str(), own.from), ("switch", "element"), "a named kind stays");
        // An equipment connection is what its mark is, when the job or the project says.
        let conn = k.resolve("equipment_connection", Some("EF"), None);
        assert_eq!((conn.kind.as_str(), conn.from), ("exhaust_fan", "project_rule"), "{conn:?}");
        let other = k.resolve("equipment_connection", Some("WH-9"), None);
        assert_eq!((other.kind.as_str(), other.from), ("equipment_connection", "element"), "an unnamed mark keeps the connection");
    }
}
