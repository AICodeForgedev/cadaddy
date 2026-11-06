//! Building knowledge as rule data (Q6210): the words the readers tell a room, a sheet, a unit,
//! a piece of equipment or a device by: room-use words (sleeping, wet, service, circulation, not a
//! room), discipline and sheet prefixes, unit-type words, equipment name words, device and fixture
//! family words.
//!
//! The lists are data, `wordrules.json` beside the engine's `coderules.json`, compiled in the same
//! way. Each list has a `source` (where the words come from and what they decide) and one shape:
//! - `words`: a list of words, in order (a reader that takes the first match keeps the order);
//! - `pairs`: `[word, value]` rows (`["FP", "FP"]`: a sheet prefix and its discipline);
//! - `groups`: `{value, words}` rows (`{"value": "E", "words": ["ELECTRICAL", "POWER PLAN"]}`);
//! - `rules`: ordered `{value, also?, any?, all?, none?}` rows, the first that matches wins: a row
//!   matches when the text has one of `any` (or `any` is empty), all of `all` and none of `none`
//!   (`{"value": "switch", "also": "IfcSwitchingDevice", "any": ["SWITCH"], "none": ["DISCONNECT"]}`).
//!   What "has" means (a substring, a whole word) is the reader's, as before.
//!
//! A job adds words the way it changes other rule data: a project rule (domain `custom`) whose key
//! is `words:<list>` (`words:rooms.sleeping`) and whose value is the words to add: a JSON list
//! (`["DORM", "BUNK ROOM"]`) or text split at `;`, `,` or a new line (`DORM; BUNK ROOM`). A
//! `pairs` list takes `WORD=VALUE`, a `groups` list `VALUE=WORD` (a value not in the list yet is a
//! new group at its end); a `rules` list `VALUE=WORD` too (the word joins the first row of that value's
//! `any`; a value not in the list yet is a new row at its end). Added words come after the shipped ones, so what the shipped words
//! decide doesn't change; a word already in the list isn't added twice. The engine installs a
//! drawing's additions before each command ([`install`]); a guided job's answer or learned rule
//! that adds a word is such a project rule.
//!
//! Fallbacks, never a panic: a list the code asks for that the data doesn't have is empty (the
//! words it would have matched match nothing) and is reported ([`report`] `missing`); a key in a
//! job's additions that no list has, or a value that can't be read, is reported (`unknown`,
//! `problems`) and changes nothing. The shipped data is checked by the engine's
//! `tests/wordrules.rs`: every list the code asks for is there with its shape, every list there is
//! one the code asks for.
//!
//! The tables are `'static`: the shipped one is built once; a job's is built once per distinct
//! set of additions (at most [`MAX_ADDED`] words, kept for reuse), so the readers keep plain
//! `&'static [&'static str]` lists. One process has one set of additions in force: the drawing the
//! last command ran on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// The shipped lists.
const DATA: &str = include_str!("../../engine/src/cmd/wordrules.json");
/// The data file's name (for sources and messages).
pub const FILE: &str = "wordrules.json";
/// The project-rule key prefix that adds words to a list (`words:rooms.sleeping`).
pub const RULE_PREFIX: &str = "words:";
/// Most words a job adds, all lists together; longest word, characters.
pub const MAX_ADDED: usize = 2_000;
pub const MAX_WORD_CHARS: usize = 120;
/// Job tables kept for reuse; missing lists remembered for the report.
const MAX_TABLES: usize = 16;
const MAX_MISSING: usize = 256;

/// A `pairs` row: a word and its value.
pub type Pair = (&'static str, &'static str);
/// A `groups` row: a value and its words.
pub type Group = (&'static str, &'static [&'static str]);

/// A `rules` row: its value (and a second one, `also`) when the text has one of `any` (or `any`
/// is empty), all of `all` and none of `none`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub value: &'static str,
    pub also: &'static str,
    pub any: &'static [&'static str],
    pub all: &'static [&'static str],
    pub none: &'static [&'static str],
}

impl Rule {
    /// Whether the row matches a text, `has` saying whether the text has a word.
    pub fn matches(&self, has: impl Fn(&str) -> bool) -> bool {
        (self.any.is_empty() || self.any.iter().any(|w| has(w))) && self.all.iter().all(|w| has(w)) && !self.none.iter().any(|w| has(w))
    }
}

/// A list's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Words,
    Pairs,
    Groups,
    Rules,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Words => "words",
            Shape::Pairs => "pairs",
            Shape::Groups => "groups",
            Shape::Rules => "rules",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileData {
    lists: BTreeMap<String, Entry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Entry {
    source: String,
    words: Option<Vec<String>>,
    pairs: Option<Vec<[String; 2]>>,
    groups: Option<Vec<GroupEntry>>,
    rules: Option<Vec<RuleEntry>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RuleEntry {
    value: String,
    also: String,
    any: Vec<String>,
    all: Vec<String>,
    none: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GroupEntry {
    value: String,
    words: Vec<String>,
}

/// The lists in force: the shipped data, plus a job's additions.
#[derive(Clone, Debug, Default)]
pub struct Table {
    words: HashMap<&'static str, &'static [&'static str]>,
    pairs: HashMap<&'static str, &'static [Pair]>,
    groups: HashMap<&'static str, &'static [Group]>,
    rules: HashMap<&'static str, &'static [Rule]>,
    sources: BTreeMap<&'static str, &'static str>,
    /// What a job added, per list (as added).
    added: BTreeMap<String, Vec<String>>,
    /// Additions for lists the data doesn't have.
    unknown: Vec<String>,
    /// Data or additions that couldn't be read.
    problems: Vec<String>,
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn leak_slice<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

fn leak_words(v: &[String]) -> &'static [&'static str] {
    leak_slice(v.iter().map(|w| leak_str(w)).collect())
}

impl Table {
    /// The table of a data file's text (the shipped one is [`builtin`]).
    pub fn parse(text: &str) -> Table {
        let mut t = Table::default();
        let data: FileData = match serde_json::from_str(text) {
            Ok(d) => d,
            Err(e) => {
                t.problems.push(format!("{FILE} does not read: {e}"));
                return t;
            }
        };
        for (key, e) in data.lists {
            let k = leak_str(&key);
            let shapes =
                usize::from(e.words.is_some()) + usize::from(e.pairs.is_some()) + usize::from(e.groups.is_some()) + usize::from(e.rules.is_some());
            if shapes != 1 {
                t.problems.push(format!("{key}: a list has exactly one of words, pairs, groups, rules ({shapes} given)"));
                continue;
            }
            if e.source.trim().is_empty() {
                t.problems.push(format!("{key}: no source"));
            }
            t.sources.insert(k, leak_str(&e.source));
            if let Some(w) = e.words {
                t.words.insert(k, leak_words(&w));
            } else if let Some(p) = e.pairs {
                t.pairs.insert(k, leak_slice(p.iter().map(|[a, b]| (leak_str(a), leak_str(b))).collect()));
            } else if let Some(g) = e.groups {
                let rows: Vec<Group> = g.iter().map(|g| (leak_str(&g.value), leak_words(&g.words))).collect();
                t.groups.insert(k, leak_slice(rows));
            } else if let Some(r) = e.rules {
                let rows: Vec<Rule> = r
                    .iter()
                    .map(|r| Rule {
                        value: leak_str(&r.value),
                        also: leak_str(&r.also),
                        any: leak_words(&r.any),
                        all: leak_words(&r.all),
                        none: leak_words(&r.none),
                    })
                    .collect();
                t.rules.insert(k, leak_slice(rows));
            }
        }
        t
    }

    /// A list's shape, if the table has it.
    pub fn shape(&self, key: &str) -> Option<Shape> {
        if self.words.contains_key(key) {
            Some(Shape::Words)
        } else if self.pairs.contains_key(key) {
            Some(Shape::Pairs)
        } else if self.groups.contains_key(key) {
            Some(Shape::Groups)
        } else if self.rules.contains_key(key) {
            Some(Shape::Rules)
        } else {
            None
        }
    }

    /// Every list's key and shape, in key order.
    pub fn keys(&self) -> Vec<(&'static str, Shape)> {
        let mut v: Vec<(&'static str, Shape)> = self.words.keys().map(|k| (*k, Shape::Words)).collect();
        v.extend(self.pairs.keys().map(|k| (*k, Shape::Pairs)));
        v.extend(self.groups.keys().map(|k| (*k, Shape::Groups)));
        v.extend(self.rules.keys().map(|k| (*k, Shape::Rules)));
        v.sort();
        v
    }

    /// A list's source text.
    pub fn source(&self, key: &str) -> &'static str {
        self.sources.get(key).copied().unwrap_or("")
    }

    pub fn words(&self, key: &str) -> Option<&'static [&'static str]> {
        self.words.get(key).copied()
    }

    pub fn pairs(&self, key: &str) -> Option<&'static [Pair]> {
        self.pairs.get(key).copied()
    }

    pub fn groups(&self, key: &str) -> Option<&'static [Group]> {
        self.groups.get(key).copied()
    }

    pub fn rules(&self, key: &str) -> Option<&'static [Rule]> {
        self.rules.get(key).copied()
    }

    /// Data or additions that couldn't be read, and additions for lists the data doesn't have.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// What a job added, per list.
    pub fn added(&self) -> &BTreeMap<String, Vec<String>> {
        &self.added
    }

    /// This table with a job's additions (`(list key, value)`, as the project rules hold them).
    pub fn with_additions(&self, additions: &[(String, String)]) -> Table {
        let mut t = self.clone();
        let mut budget = MAX_ADDED;
        for (key, value) in additions {
            let key = key.trim();
            let Some(shape) = self.shape(key) else {
                if t.unknown.len() < 100 {
                    t.unknown.push(key.chars().take(MAX_WORD_CHARS).collect());
                }
                continue;
            };
            let items = split_value(value);
            let mut added: Vec<String> = Vec::new();
            for item in items {
                if budget == 0 {
                    t.problems.push(format!("more than {MAX_ADDED} words added: the rest are left out"));
                    break;
                }
                if item.chars().count() > MAX_WORD_CHARS {
                    t.problems.push(format!("{key}: a word longer than {MAX_WORD_CHARS} characters is left out"));
                    continue;
                }
                let done = match shape {
                    Shape::Words => t.add_word(key, &item),
                    Shape::Pairs | Shape::Groups | Shape::Rules => match item.split_once('=') {
                        Some((a, b)) if !a.trim().is_empty() && !b.trim().is_empty() => match shape {
                            Shape::Pairs => t.add_pair(key, a.trim(), b.trim()),
                            Shape::Groups => t.add_to_group(key, a.trim(), b.trim()),
                            _ => t.add_to_rule(key, a.trim(), b.trim()),
                        },
                        _ => {
                            let form = if shape == Shape::Pairs { "WORD=VALUE" } else { "VALUE=WORD" };
                            t.problems.push(format!("{key}: `{}` is not {form}", item.chars().take(40).collect::<String>()));
                            false
                        }
                    },
                };
                if done {
                    budget = budget.saturating_sub(1);
                    added.push(item);
                }
            }
            if !added.is_empty() {
                t.added.entry(key.to_string()).or_default().extend(added);
            }
        }
        t
    }

    fn add_word(&mut self, key: &str, w: &str) -> bool {
        let Some((k, old)) = self.words.get_key_value(key).map(|(k, v)| (*k, *v)) else { return false };
        if old.contains(&w) {
            return false;
        }
        let mut v = old.to_vec();
        v.push(leak_str(w));
        self.words.insert(k, leak_slice(v));
        true
    }

    fn add_pair(&mut self, key: &str, a: &str, b: &str) -> bool {
        let Some((k, old)) = self.pairs.get_key_value(key).map(|(k, v)| (*k, *v)) else { return false };
        if old.iter().any(|(x, y)| *x == a && *y == b) {
            return false;
        }
        let mut v = old.to_vec();
        v.push((leak_str(a), leak_str(b)));
        self.pairs.insert(k, leak_slice(v));
        true
    }

    fn add_to_group(&mut self, key: &str, value: &str, w: &str) -> bool {
        let Some((k, old)) = self.groups.get_key_value(key).map(|(k, v)| (*k, *v)) else { return false };
        let mut rows = old.to_vec();
        match rows.iter_mut().find(|(v, _)| *v == value) {
            Some((_, words)) if words.contains(&w) => return false,
            Some((_, words)) => {
                let mut ws = words.to_vec();
                ws.push(leak_str(w));
                *words = leak_slice(ws);
            }
            None => rows.push((leak_str(value), leak_slice(vec![leak_str(w)]))),
        }
        self.groups.insert(k, leak_slice(rows));
        true
    }

    fn add_to_rule(&mut self, key: &str, value: &str, w: &str) -> bool {
        let Some((k, old)) = self.rules.get_key_value(key).map(|(k, v)| (*k, *v)) else { return false };
        let mut rows = old.to_vec();
        match rows.iter_mut().find(|r| r.value == value) {
            Some(r) if r.any.contains(&w) => return false,
            Some(r) => {
                let mut ws = r.any.to_vec();
                ws.push(leak_str(w));
                r.any = leak_slice(ws);
            }
            None => rows.push(Rule { value: leak_str(value), any: leak_slice(vec![leak_str(w)]), ..Rule::default() }),
        }
        self.rules.insert(k, leak_slice(rows));
        true
    }
}

/// The items of an addition's value: a JSON list of strings, else text split at `;`, `,` or a
/// new line; blank items dropped.
fn split_value(value: &str) -> Vec<String> {
    let v = value.trim();
    if v.starts_with('[')
        && let Ok(list) = serde_json::from_str::<Vec<String>>(v)
    {
        return list.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
    v.split([';', ',', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The shipped table.
pub fn builtin() -> &'static Table {
    static T: OnceLock<&'static Table> = OnceLock::new();
    T.get_or_init(|| Box::leak(Box::new(Table::parse(DATA))))
}

static HAS_JOB: AtomicBool = AtomicBool::new(false);
static ACTIVE: RwLock<Option<&'static Table>> = RwLock::new(None);
/// The additions in force and the tables built for earlier ones.
static INSTALLED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static TABLES: Mutex<Vec<(Vec<(String, String)>, &'static Table)>> = Mutex::new(Vec::new());
static MISSING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The table in force: the shipped one, or a job's.
pub fn table() -> &'static Table {
    if !HAS_JOB.load(Ordering::Acquire) {
        return builtin();
    }
    ACTIVE.read().ok().and_then(|g| *g).unwrap_or_else(builtin)
}

/// Put a job's additions in force (`(list key, value)`; none: the shipped lists). The same
/// additions as before change nothing.
pub fn install(additions: &[(String, String)]) {
    if additions.is_empty() && !HAS_JOB.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut cur) = INSTALLED.lock() else { return };
    if cur.as_slice() == additions {
        return;
    }
    if additions.is_empty() {
        HAS_JOB.store(false, Ordering::Release);
        if let Ok(mut a) = ACTIVE.write() {
            *a = None;
        }
        cur.clear();
        return;
    }
    let t: &'static Table = match TABLES.lock() {
        Ok(mut tables) => match tables.iter().find(|(a, _)| a.as_slice() == additions) {
            Some((_, t)) => t,
            None => {
                let t: &'static Table = Box::leak(Box::new(builtin().with_additions(additions)));
                if tables.len() >= MAX_TABLES {
                    tables.remove(0);
                }
                tables.push((additions.to_vec(), t));
                t
            }
        },
        Err(_) => return,
    };
    if let Ok(mut a) = ACTIVE.write() {
        *a = Some(t);
        HAS_JOB.store(true, Ordering::Release);
        *cur = additions.to_vec();
    }
}

fn note_missing(key: &str, shape: Shape) {
    if let Ok(mut m) = MISSING.lock()
        && m.len() < MAX_MISSING
    {
        m.insert(format!("{key} ({})", shape.as_str()));
    }
}

/// A `words` list (empty, and reported, if the data doesn't have it).
pub fn words(key: &str) -> &'static [&'static str] {
    table().words(key).unwrap_or_else(|| {
        note_missing(key, Shape::Words);
        &[]
    })
}

/// A `pairs` list (empty, and reported, if the data doesn't have it).
pub fn pairs(key: &str) -> &'static [Pair] {
    table().pairs(key).unwrap_or_else(|| {
        note_missing(key, Shape::Pairs);
        &[]
    })
}

/// A `groups` list (empty, and reported, if the data doesn't have it).
pub fn groups(key: &str) -> &'static [Group] {
    table().groups(key).unwrap_or_else(|| {
        note_missing(key, Shape::Groups);
        &[]
    })
}

/// A `rules` list (empty, and reported, if the data doesn't have it).
pub fn rules(key: &str) -> &'static [Rule] {
    table().rules(key).unwrap_or_else(|| {
        note_missing(key, Shape::Rules);
        &[]
    })
}

/// The first row of a `rules` list that matches, `has` saying whether the text has a word.
pub fn first_rule(key: &str, has: impl Fn(&str) -> bool) -> Option<&'static Rule> {
    rules(key).iter().find(|r| r.matches(&has))
}

/// One group's words (none when the list has no such value).
pub fn group(key: &str, value: &str) -> &'static [&'static str] {
    groups(key).iter().find(|(v, _)| *v == value).map_or(&[], |(_, w)| *w)
}

/// The value of the first pair whose word is `word`.
pub fn pair_value(key: &str, word: &str) -> Option<&'static str> {
    pairs(key).iter().find(|(w, _)| *w == word).map(|(_, v)| *v)
}

/// What the lists in force are and what went wrong with them.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    /// The data file and how many lists it has.
    pub file: &'static str,
    pub lists: usize,
    /// Words a job added, per list.
    pub added: BTreeMap<String, Vec<String>>,
    /// Additions for lists the data doesn't have.
    pub unknown: Vec<String>,
    /// Data or additions that couldn't be read.
    pub problems: Vec<String>,
    /// Lists the code asked for that the data doesn't have (they matched nothing).
    pub missing: Vec<String>,
}

pub fn report() -> Report {
    let t = table();
    let mut problems = builtin().problems.clone();
    problems.extend(t.problems.iter().filter(|p| !problems.contains(p)).cloned().collect::<Vec<_>>());
    Report {
        file: FILE,
        lists: t.keys().len(),
        added: t.added.clone(),
        unknown: t.unknown.clone(),
        problems,
        missing: MISSING.lock().map(|m| m.iter().cloned().collect()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"lists": {
        "rooms.sleep": {"source": "test", "words": ["BUNK", "LOFT"]},
        "sheets.prefix": {"source": "test", "pairs": [["FP", "FP"], ["E", "E"]]},
        "sheets.title": {"source": "test", "groups": [{"value": "E", "words": ["ELECTRICAL"]}]},
        "devices.legend": {"source": "test", "rules": [
            {"value": "ceiling_receptacle", "also": "IfcOutlet", "all": ["CEILING", "RECEPTACLE"]},
            {"value": "switch", "any": ["SWITCH"], "none": ["DISCONNECT"]},
            {"value": "disconnect", "any": ["DISCONNECT"]}
        ]}
    }}"#;

    fn adds(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn the_shipped_data_reads_with_no_problem() {
        let t = builtin();
        assert!(t.problems().is_empty(), "{:?}", t.problems());
        assert!(t.keys().len() > 40, "{}", t.keys().len());
        for (k, _) in t.keys() {
            assert!(!t.source(k).trim().is_empty(), "{k} has no source");
        }
    }

    #[test]
    fn a_list_reads_in_order_with_its_shape() {
        let t = Table::parse(SAMPLE);
        assert!(t.problems().is_empty(), "{:?}", t.problems());
        assert_eq!(t.words("rooms.sleep"), Some(&["BUNK", "LOFT"][..]));
        assert_eq!(t.pairs("sheets.prefix").map(<[Pair]>::len), Some(2));
        assert_eq!(t.shape("sheets.title"), Some(Shape::Groups));
        assert_eq!(t.shape("nope"), None);
        assert_eq!(t.source("rooms.sleep"), "test");
    }

    #[test]
    fn a_job_adds_words_after_the_shipped_ones_once() {
        let t = Table::parse(SAMPLE).with_additions(&adds(&[
            ("rooms.sleep", r#"["DORM", "BUNK"]"#),
            ("rooms.sleep", "BUNK ROOM; DORM"),
            ("sheets.prefix", "X=E"),
            ("sheets.title", "E=POWER; T=TELECOM"),
        ]));
        assert_eq!(t.words("rooms.sleep"), Some(&["BUNK", "LOFT", "DORM", "BUNK ROOM"][..]));
        assert_eq!(t.pairs("sheets.prefix").and_then(|p| p.last()).copied(), Some(("X", "E")));
        let g = t.groups("sheets.title").unwrap_or_default();
        assert_eq!(g.first().map(|(_, w)| w.to_vec()), Some(vec!["ELECTRICAL", "POWER"]));
        assert_eq!(g.get(1).map(|(v, w)| (*v, w.to_vec())), Some(("T", vec!["TELECOM"])));
        assert_eq!(t.added().get("rooms.sleep"), Some(&vec!["DORM".to_string(), "BUNK ROOM".to_string()]));
        assert!(t.unknown().is_empty() && t.problems().is_empty(), "{:?} {:?}", t.unknown(), t.problems());
    }

    #[test]
    fn the_first_rule_that_matches_wins() {
        let t = Table::parse(SAMPLE);
        let first =
            |text: &str| t.rules("devices.legend").unwrap_or_default().iter().find(|r| r.matches(|w| text.contains(w))).map(|r| (r.value, r.also));
        assert_eq!(first("CEILING MOUNTED RECEPTACLE"), Some(("ceiling_receptacle", "IfcOutlet")));
        assert_eq!(first("RECEPTACLE"), None);
        assert_eq!(first("SWITCH"), Some(("switch", "")));
        assert_eq!(first("FUSED DISCONNECT SWITCH"), Some(("disconnect", "")));
        let t = t.with_additions(&adds(&[("devices.legend", "switch=TOGGLE; dimmer=DIMMER")]));
        let rows = t.rules("devices.legend").unwrap_or_default();
        assert_eq!(rows.get(1).map(|r| r.any.to_vec()), Some(vec!["SWITCH", "TOGGLE"]));
        assert_eq!(rows.last().map(|r| (r.value, r.any.to_vec())), Some(("dimmer", vec!["DIMMER"])));
    }

    #[test]
    fn an_unknown_list_or_a_bad_value_is_reported_and_changes_nothing() {
        let base = Table::parse(SAMPLE);
        let t = base.with_additions(&adds(&[("rooms.nope", "X"), ("sheets.prefix", "NOEQUALS"), ("sheets.title", "=X")]));
        assert_eq!(t.unknown(), &["rooms.nope".to_string()]);
        assert_eq!(t.problems().len(), 2, "{:?}", t.problems());
        assert_eq!(t.pairs("sheets.prefix"), base.pairs("sheets.prefix"));
        assert!(t.added().is_empty());
    }

    #[test]
    fn broken_data_is_a_problem_not_a_panic() {
        assert!(!Table::parse("{not json").problems().is_empty());
        let t = Table::parse(r#"{"lists": {"a": {"source": "s", "words": ["X"], "pairs": [["A", "B"]]}, "b": {"words": ["Y"]}}}"#);
        assert_eq!(t.shape("a"), None);
        assert_eq!(t.words("b"), Some(&["Y"][..]));
        assert_eq!(t.problems().len(), 2, "{:?}", t.problems());
    }

    #[test]
    fn additions_are_capped() {
        let many: Vec<String> = (0..MAX_ADDED + 10).map(|i| format!("W{i}")).collect();
        let long = "X".repeat(MAX_WORD_CHARS + 1);
        let t = Table::parse(SAMPLE).with_additions(&[("rooms.sleep".into(), many.join(";")), ("rooms.sleep".into(), long)]);
        assert_eq!(t.words("rooms.sleep").map(<[&str]>::len), Some(2 + MAX_ADDED));
        assert!(!t.problems().is_empty());
    }

    #[test]
    fn a_list_the_data_lacks_is_empty_and_reported() {
        assert!(words("no.such.list").is_empty());
        assert!(pairs("no.such.pairs").is_empty());
        assert!(group("no.such.groups", "E").is_empty());
        let r = report();
        assert!(r.missing.iter().any(|m| m == "no.such.list (words)"), "{:?}", r.missing);
        assert_eq!(r.file, FILE);
    }
}
