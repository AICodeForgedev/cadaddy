//! Whose work each element is (Q6167 `scopesplit`): for each class of element another trade may
//! furnish or install (thermostats, fire alarm devices, low voltage devices), the party that
//! furnishes it and the one that installs it, and what of its rough-in is ours, each decided from
//! the documents with the words that say so.
//!
//! Read from the sheets' notes (numbered lists under a `NOTES` heading, and the loose notes of a
//! legend), the specification sections (a section in Division 23 speaks for the mechanical
//! trade), the submittals and any responsibility matrix (`ITEM | FURNISHED BY | INSTALLED BY`).
//! A sentence says who does it by a party's name (`FIRE ALARM CONTRACTOR SHALL PROVIDE…`,
//! `…FURNISHED BY THE MECHANICAL CONTRACTOR`, `…ARE THE RESPONSIBILITY OF THE FIRE ALARM
//! CONTRACTOR`), by the trade of its document when it gives an order with no one named
//! (`Install thermostats in outlet boxes…` in a Division 23 section), and what is ours by its
//! rough-in words (`ROUGH-IN ONLY. PROVIDE A BOX AND 1" CONDUIT`, `NO BOX IS
//! REQUIRED`). The words are data ([`scope_rules.json`]), never one job's.
//!
//! The decisions are kept in the project rules (`scope.<class>.party`, `scope.<class>.ours`) at
//! the tier of the document that said them: a later reading replaces its own tier, a person's
//! answer outranks a default and a document outranks a person's guess. A class no document
//! speaks for takes the data's default, and `review.list` asks about it.
//!
//! `bim.scope` reads the documents (the drawing's sheets, and specifications, submittals and
//! notes given), records the decisions, gives the fire alarm devices their kinds from the
//! legends and lists who does what; `bim.bom` keeps only our material in its lines and lists
//! the rest in `by_others`.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::projrules::{RuleDomain, RuleValue, SourceTier};
use cadaddy_mepfix::sheet_facts::TextItem;
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.scope";
const RULES: &str = include_str!("scope_rules.json");
/// Paragraphs read from one document, statements kept, characters of one paragraph.
const MAX_PARAS: usize = 20_000;
const MAX_STATEMENTS: usize = 20_000;
const MAX_PARA_CHARS: usize = 4_000;
/// Files read in one call, pages of one drawing PDF.
const MAX_FILES: usize = 400;
const MAX_PDF_PAGES: u64 = 500;
/// Statements an `explain` answer lists.
const MAX_EXPLAIN: usize = 300;
/// Characters of a citation's quoted words.
const QUOTE: usize = 240;
/// Words of a sentence at most: more is a table's cells run together, not a note.
const MAX_SENTENCE: usize = 50;
/// Tokens a construct may reach over (a party's name to its verb, `NO` to its `REQUIRED`).
const REACH: usize = 12;

// ─── The rules data ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Party {
    pub label: String,
    pub words: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Division {
    pub prefix: String,
    pub party: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Class {
    pub class: String,
    pub label: String,
    pub party: String,
    pub kinds: Vec<String>,
    pub words: Vec<String>,
    pub system_words: Vec<String>,
    pub default_ours: Vec<String>,
    pub default_reason: String,
    /// Kinds (or fire alarm kinds) the `power` item is for (a panel's circuit, not a device's).
    pub power_kinds: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FaKind {
    pub kind: String,
    /// Each group: one of its words must be there.
    pub all: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PanelRule {
    pub kind: String,
    pub tag: String,
    pub words: Vec<String>,
    /// Width, depth, height, inches.
    pub size_in: [f64; 3],
    pub top_in: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub ours: String,
    pub parties: BTreeMap<String, Party>,
    pub divisions: Vec<Division>,
    pub disciplines: BTreeMap<String, String>,
    pub classes: Vec<Class>,
    pub items: BTreeMap<String, Vec<String>>,
    pub item_names: BTreeMap<String, String>,
    pub whole_system: Vec<String>,
    pub design_build: Vec<String>,
    pub rough_in_only: Vec<String>,
    pub fa_kinds: Vec<FaKind>,
    pub panels: Vec<PanelRule>,
    pub panel_source: String,
}

/// The rules data, read once.
pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

impl Rules {
    fn label(&self, party: &str) -> String {
        self.parties.get(party).map_or_else(|| party.to_string(), |p| p.label.clone())
    }

    /// A party by its key or any of its words (`mechanical`, `MC`, `fire alarm contractor`).
    pub(crate) fn party_named(&self, said: &str) -> Option<String> {
        let t = toks(said);
        let k = said.trim().to_ascii_lowercase();
        if self.parties.contains_key(&k) {
            return Some(k);
        }
        self.parties.iter().find(|(_, p)| p.words.iter().any(|w| toks(w) == t) || p.label.eq_ignore_ascii_case(said.trim())).map(|(k, _)| k.clone())
    }

    /// The trade a specification section speaks for, by its number (`23 09 00`).
    fn division_party(&self, section: &str) -> Option<String> {
        let digits: String = section.chars().filter(char::is_ascii_digit).take(6).collect();
        if digits.len() < 2 {
            return None;
        }
        self.divisions.iter().filter(|d| digits.starts_with(&d.prefix)).max_by_key(|d| d.prefix.len()).map(|d| d.party.clone())
    }

    /// The trade a sheet speaks for, by its discipline (`M`, `FA`) or its number's letters.
    pub(crate) fn discipline_party(&self, sheet: &str) -> Option<String> {
        let letters: String = sheet.trim().chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
        self.disciplines.get(&letters).cloned()
    }

    /// The class an element of `kind` (its type words `what` as a fallback) is, if another trade
    /// may own it.
    pub(crate) fn class_of(&self, kind: &str, what: &str) -> Option<&Class> {
        let k = kind.replace('_', " ").trim().to_ascii_lowercase();
        let by_kind = self.classes.iter().find(|c| c.kinds.iter().any(|x| x.eq_ignore_ascii_case(&k)));
        if by_kind.is_some() || !matches!(k.as_str(), "device" | "" | "equipment" | "equipment connection") {
            return by_kind;
        }
        let w = what.replace('_', " ").to_ascii_lowercase();
        self.classes.iter().find(|c| c.kinds.iter().any(|x| has_words(&w, x)))
    }

    fn class(&self, name: &str) -> Option<&Class> {
        self.classes.iter().find(|c| c.class.eq_ignore_ascii_case(name.trim()))
    }

    /// A fire alarm device's kind from what its legend row (or its name) says.
    pub(crate) fn fa_kind_of(&self, words: &str) -> Option<String> {
        // `SMOKE/CARBON MONOXIDE`, `HORN/STROBE`: each word apart.
        let apart = |w: &str| toks(&w.replace('/', " "));
        let t = apart(words);
        self.fa_kinds
            .iter()
            .find(|k| !k.all.is_empty() && k.all.iter().all(|g| g.iter().any(|w| find(&t, &apart(w)).is_some())))
            .map(|k| k.kind.clone())
    }

    pub(crate) fn item_name(&self, item: &str) -> String {
        self.item_names.get(item).cloned().unwrap_or_else(|| item.to_string())
    }
}

// ─── Words ──────────────────────────────────────────────────────────────────

/// A text's words: upper case, quotes straightened, the punctuation round each word dropped (a
/// word's own dots, dashes, slashes and inch marks kept: `E.C`, `ROUGH-IN`, `3/4"`); two words
/// an import ran together (`REQUIREDFOR`, `VOLTAGEVENDOR`) apart again where both are words the
/// rules read by.
fn toks(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in t
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
        .to_ascii_uppercase()
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '!' | '?' | '*'))
        .map(|w| w.trim_matches(|c: char| c == '.' || c == '\'' || c == '-' || c == '&').trim_start_matches('"'))
        .filter(|w| !w.is_empty())
    {
        match unglue(w) {
            Some((a, b)) => {
                out.push(a.to_string());
                out.push(b.to_string());
            }
            None => out.push(w.to_string()),
        }
    }
    out
}

/// The words the rules read by (their phrases' words and the statement words).
fn vocabulary() -> &'static std::collections::HashSet<String> {
    static V: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let r = rules();
        let mut v: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut put = |p: &str| {
            for w in p.to_ascii_uppercase().split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '/')) {
                if w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic()) {
                    v.insert(w.to_string());
                }
            }
        };
        for p in r.parties.values() {
            p.words.iter().for_each(|w| put(w));
        }
        for c in &r.classes {
            c.words.iter().chain(c.system_words.iter()).for_each(|w| put(w));
        }
        r.items.values().flatten().chain(r.design_build.iter()).chain(r.rough_in_only.iter()).chain(r.whole_system.iter()).for_each(|w| put(w));
        for w in VERBS.iter().chain(PARTICIPLES.iter()).chain(STOPS.iter()) {
            put(w);
        }
        for w in [
            "REQUIRED",
            "NEEDED",
            "SHALL",
            "WILL",
            "MUST",
            "BY",
            "THE",
            "AND",
            "OR",
            "NOT",
            "RESPONSIBILITY",
            "OF",
            "IS",
            "ARE",
            "BE",
            "ONLY",
            "NO",
            "ALL",
            "WITH",
        ] {
            put(w);
        }
        v
    })
}

/// A word an import ran two of the rules' words together into, apart (`REQUIREDFOR`); a number
/// and the word after it (`605AND`: a room's number, then AND).
fn unglue(w: &str) -> Option<(&str, &str)> {
    let v = vocabulary();
    let digits = w.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && w.len() - digits >= 2 {
        let (n, rest) = (w.get(..digits)?, w.get(digits..)?);
        return (rest.chars().all(|c| c.is_ascii_alphabetic()) && v.contains(rest)).then_some((n, rest));
    }
    if w.len() < 5 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if v.contains(w) {
        return None;
    }
    (2..=w.len() - 2).find_map(|i| {
        let (a, b) = (w.get(..i)?, w.get(i..)?);
        (v.contains(a) && v.contains(b) && a.len() + b.len() >= 5).then_some((a, b))
    })
}

/// Where `phrase` starts among `words`.
fn find(words: &[String], phrase: &[String]) -> Option<usize> {
    find_from(words, phrase, 0)
}

fn find_from(words: &[String], phrase: &[String], from: usize) -> Option<usize> {
    if phrase.is_empty() || phrase.len() > words.len() {
        return None;
    }
    (from..=words.len() - phrase.len()).find(|&i| words.get(i..i + phrase.len()).is_some_and(|w| w == phrase))
}

/// Does `hay` hold `phrase` as whole words?
fn has_words(hay: &str, phrase: &str) -> bool {
    find(&toks(hay), &toks(phrase)).is_some()
}

/// The first of `phrases` found among `words` (the longest at a place first): (start, end, which).
fn spans<'a>(words: &[String], phrases: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(usize, usize, String)> {
    let mut all: Vec<(Vec<String>, String)> = phrases.map(|(p, key)| (toks(p), key.to_string())).filter(|(p, _)| !p.is_empty()).collect();
    all.sort_by_key(|b| std::cmp::Reverse(b.0.len()));
    let mut taken = vec![false; words.len()];
    let mut out = Vec::new();
    for (p, key) in &all {
        let mut from = 0;
        while let Some(i) = find_from(words, p, from) {
            let end = i + p.len();
            if taken.get(i..end).is_some_and(|t| t.iter().all(|x| !x)) {
                for t in taken.iter_mut().take(end).skip(i) {
                    *t = true;
                }
                out.push((i, end, key.clone()));
            }
            from = i + 1;
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

/// A raw word that ends a sentence with its dot (`SHOP DRAWINGS.`, not `E.C.`, `NO.` or `1.`).
fn ends_sentence(raw: &str) -> bool {
    let t = raw.trim_end_matches(['"', '\'', ')']);
    let Some(body) = t.strip_suffix('.') else { return false };
    let body = body.trim_start_matches(['(', '"']);
    let abbrev = ["NO", "FL", "DIV", "ST", "STE", "MIN", "MAX", "APPROX", "TYP", "EX", "SEC", "REF", "RM", "BLDG", "MFR", "MFG", "EQ", "SIM"];
    body.chars().count() > 2
        && !body.contains('.')
        && !body.chars().all(|c| c.is_ascii_digit())
        && !abbrev.contains(&body.to_ascii_uppercase().as_str())
}

/// A paragraph's leading outline mark (`A.`, `1.`, `2)`, `a.`), if it starts with one.
fn outline_mark(raw: &str) -> bool {
    let core = raw.trim_end_matches(['.', ')']);
    raw.len() > core.len()
        && !core.is_empty()
        && core.len() <= 3
        && (core.chars().all(|c| c.is_ascii_digit() || c == '.') || (core.len() == 1 && core.chars().all(|c| c.is_ascii_alphabetic())))
}

/// A paragraph's sentences, each as its words (its outline mark left out).
fn sentences(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut words = text.split_whitespace().peekable();
    if words.peek().is_some_and(|w| outline_mark(w)) {
        words.next();
    }
    for raw in words.take(2_000) {
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(raw);
        if ends_sentence(raw) {
            out.push(toks(&cur));
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(toks(&cur));
    }
    out.retain(|s| !s.is_empty());
    out
}

// ─── Documents ──────────────────────────────────────────────────────────────

/// One paragraph of a document: a note, a section's article, a matrix row's cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Para {
    /// Where in the document (`note 3`, `262726 - 2`).
    pub at: String,
    /// The heading it sits under (`FIRE ALARM SYSTEM GENERAL NOTES`).
    pub heading: String,
    pub text: String,
    /// A table row's cells, left to right (a responsibility matrix's).
    pub cells: Vec<String>,
}

/// A document read for who does what.
#[derive(Clone, Debug, Default)]
pub(crate) struct Doc {
    /// `E-301`, `23 09 00 Controls`, a submittal's file name.
    pub name: String,
    pub tier: SourceTier,
    /// The trade an order with no one named is given to (a mechanical sheet's, a Division 23
    /// section's).
    pub trade: Option<String>,
    pub paras: Vec<Para>,
}

/// A sheet's notes as paragraphs: each numbered note of a list under a `NOTES` heading, the
/// loose notes (lines run on one under another), and the rows of a table whose header names
/// who furnishes and who installs.
pub(crate) fn sheet_doc(number: &str, texts: &[TextItem]) -> Doc {
    let r = rules();
    let mut doc = Doc { name: number.to_string(), tier: SourceTier::DrawingRevision, trade: r.discipline_party(number), paras: Vec::new() };
    let runs = join_runs(texts);
    let lists = super::keynotes::lists_under(&runs, |u| u.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "NOTES" || w == "KEYNOTES"));
    let mut listed: Vec<&str> = Vec::new();
    for l in &lists {
        for n in &l.notes {
            if doc.paras.len() < MAX_PARAS {
                doc.paras.push(Para {
                    at: format!("{} note {}", l.heading.trim_end_matches(':'), n.number),
                    heading: l.heading.clone(),
                    text: n.text.clone(),
                    cells: Vec::new(),
                });
            }
        }
    }
    for p in &doc.paras {
        listed.push(p.text.as_str());
    }
    let loose = loose_paragraphs(&runs);
    let listed_owned: Vec<String> = listed.iter().map(|s| s.to_string()).collect();
    for (at, text) in loose {
        if doc.paras.len() >= MAX_PARAS {
            break;
        }
        // A list's note read again line by line (its number with it): once is enough.
        let bare = {
            let t = text.trim_start();
            let n = t.chars().take_while(char::is_ascii_digit).count();
            match t.get(n..) {
                Some(r) if (1..=2).contains(&n) && (r.starts_with('.') || r.starts_with(')')) => r.get(1..).unwrap_or_default().trim_start(),
                _ => t,
            }
        };
        let first: String = bare.chars().take(40).collect();
        if listed_owned.iter().any(|n| n.contains(first.trim())) {
            continue;
        }
        doc.paras.push(Para { at, heading: String::new(), text, cells: Vec::new() });
    }
    doc.paras.extend(matrix_rows_of_sheet(texts));
    doc.paras.truncate(MAX_PARAS);
    doc
}

/// A sheet's text with the runs of one line joined (an import splits a line where its font or
/// spacing changes: `PROVIDE WITH "X" THERMOSTAT.` and `NO CONDUIT … REQUIRED.`): the next run on
/// the same baseline, as tall, starting about where the last one's letters end.
pub(crate) fn join_runs(texts: &[TextItem]) -> Vec<TextItem> {
    let mut flat: Vec<TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.is_finite()).take(50_000).cloned().collect();
    flat.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let mut out: Vec<TextItem> = Vec::with_capacity(flat.len());
    for t in flat {
        if let Some(last) = out.last_mut() {
            let h = last.height;
            let n = last.value.chars().count() as f64;
            let same_line = (last.at.y - t.at.y).abs() <= 0.2 * h && (t.height - h).abs() <= 0.2 * h;
            let from = last.at.x + 0.35 * h * n;
            let to = last.at.x + 0.85 * h * n + 1.5 * h;
            if same_line && t.at.x >= from && t.at.x <= to && last.value.len() + t.value.len() <= MAX_PARA_CHARS {
                last.value.push(' ');
                last.value.push_str(t.value.trim());
                continue;
            }
        }
        out.push(t);
    }
    out.extend(texts.iter().filter(|t| !(t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.is_finite())).cloned());
    out
}

/// The loose notes of a sheet: each run of lines one under another (left-aligned, about a line
/// apart), joined; a numbered line starts a new one.
fn loose_paragraphs(texts: &[TextItem]) -> Vec<(String, String)> {
    let lines: Vec<&TextItem> = texts
        .iter()
        .filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.is_finite() && t.value.split_whitespace().count() >= 2)
        .take(50_000)
        .collect();
    // Top down.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| {
        let (Some(x), Some(y)) = (lines.get(a), lines.get(b)) else { return std::cmp::Ordering::Equal };
        y.at.y.total_cmp(&x.at.y).then(x.at.x.total_cmp(&y.at.x))
    });
    let ys: Vec<f64> = order.iter().filter_map(|&i| lines.get(i)).map(|t| t.at.y).collect();
    let starts_number = |v: &str| {
        let n: String = v.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        !n.is_empty() && n.len() <= 2 && v.trim().get(n.len()..).is_some_and(|r| r.starts_with('.') || r.starts_with(')'))
    };
    let mut next: Vec<Option<usize>> = vec![None; order.len()];
    let mut has_prev = vec![false; order.len()];
    for (k, &i) in order.iter().enumerate() {
        let Some(t) = lines.get(i) else { continue };
        let h = t.height;
        // The lines a little below it: the y-sorted run from just under it.
        let lo = t.at.y - 1.9 * h;
        let mut best: Option<(usize, f64)> = None;
        let mut j = k + 1;
        while let Some(&y) = ys.get(j) {
            if y < lo {
                break;
            }
            if let Some(u) = order.get(j).and_then(|&o| lines.get(o))
                && t.at.y - u.at.y > 0.4 * h
                && (u.at.x - t.at.x).abs() <= 1.5 * h
                && (u.height - h).abs() <= 0.3 * h
                && !has_prev.get(j).copied().unwrap_or(true)
                && !starts_number(&u.value)
            {
                let d = t.at.y - u.at.y;
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((j, d));
                }
            }
            j += 1;
        }
        if let Some((j, _)) = best {
            if let Some(n) = next.get_mut(k) {
                *n = Some(j);
            }
            if let Some(p) = has_prev.get_mut(j) {
                *p = true;
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..order.len() {
        if has_prev.get(k).copied().unwrap_or(true) {
            continue;
        }
        let mut text = String::new();
        let mut cur = Some(k);
        let mut n = 0;
        while let Some(c) = cur {
            let Some(t) = order.get(c).and_then(|&o| lines.get(o)) else { break };
            let v = t.value.trim();
            if text.ends_with('-') && !text.ends_with(" -") {
                text.push_str(v);
            } else {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(v);
            }
            n += 1;
            if n >= 60 || text.len() >= MAX_PARA_CHARS {
                break;
            }
            cur = next.get(c).copied().flatten();
        }
        // Only what can say who does something: more than a label.
        if text.split_whitespace().count() >= 4 {
            let at = order.get(k).and_then(|&o| lines.get(o)).map_or_else(String::new, |t| format!("note at {:.0},{:.0}", t.at.x, t.at.y));
            out.push((at, text));
        }
        if out.len() >= MAX_PARAS {
            break;
        }
    }
    out
}

/// A responsibility matrix on a sheet: the rows under a header row that names who furnishes and
/// who installs (cells on one line, left to right).
fn matrix_rows_of_sheet(texts: &[TextItem]) -> Vec<Para> {
    if !texts.iter().any(|t| t.value.to_ascii_uppercase().contains("FURNISH")) {
        return Vec::new();
    }
    let flat: Vec<&TextItem> = texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.is_finite()).take(50_000).collect();
    // Rows: text on one line.
    let mut sorted = flat.clone();
    sorted.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
    let mut rows: Vec<Vec<&TextItem>> = Vec::new();
    for t in sorted {
        match rows.last_mut() {
            Some(r) if r.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.35 * f.height.max(t.height)) => r.push(t),
            _ => rows.push(vec![t]),
        }
    }
    let as_cells = |r: &Vec<&TextItem>| -> Vec<String> {
        let mut r = r.clone();
        r.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        r.iter().map(|t| t.value.trim().to_string()).collect()
    };
    let lines: Vec<Vec<String>> = rows.iter().map(as_cells).collect();
    matrix_paras(&lines, "matrix row")
}

/// The rows of each responsibility matrix among table lines (each line's cells).
fn matrix_paras(lines: &[Vec<String>], at: &str) -> Vec<Para> {
    let mut out = Vec::new();
    let mut header: Option<(String, Vec<String>)> = None;
    let mut blank = 0;
    for (i, cells) in lines.iter().enumerate() {
        let up: Vec<String> = cells.iter().map(|c| c.to_ascii_uppercase()).collect();
        let roles = up.iter().filter(|c| c.contains("FURNISH") || c.contains("INSTALL") || c.contains("PROVIDED BY") || c.contains("WIRED")).count();
        if roles >= 2 && up.iter().any(|c| c.contains("FURNISH") || c.contains("PROVIDED")) {
            header = Some((cells.join(" | "), up));
            blank = 0;
            continue;
        }
        let Some((head, cols)) = &header else { continue };
        if cells.len() < 2 {
            blank += 1;
            if blank > 2 {
                header = None;
            }
            continue;
        }
        blank = 0;
        // Each cell under its column: the header's role words before it.
        let mut text = String::new();
        if let Some(item) = cells.first() {
            text.push_str(item);
        }
        for (k, c) in cells.iter().enumerate().skip(1) {
            let role = cols.get(k + cols.len().saturating_sub(cells.len())).cloned().unwrap_or_default();
            text.push_str(&format!("; {role}: {c}"));
        }
        if out.len() < MAX_PARAS {
            out.push(Para { at: format!("{at} {}", i + 1), heading: head.clone(), text, cells: cells.clone() });
        }
    }
    out
}

/// A specification section's (or a submittal's) text as paragraphs: articles and their lettered
/// and numbered paragraphs (an outline mark on its own line starts one), table lines (cells 3
/// spaces apart) as rows. `page` marks (`262726 - 2`, `--- page N`) say where.
pub(crate) fn text_doc(name: &str, tier: SourceTier, trade: Option<String>, text: &str) -> Doc {
    let mut doc = Doc { name: name.to_string(), tier, trade, paras: Vec::new() };
    let mut page = String::new();
    let mut cur = String::new();
    let mut cur_at = String::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    let flush = |cur: &mut String, at: &str, doc: &mut Doc| {
        let t = cur.trim();
        if t.split_whitespace().count() >= 3 && doc.paras.len() < MAX_PARAS {
            doc.paras.push(Para { at: at.to_string(), heading: String::new(), text: t.chars().take(MAX_PARA_CHARS).collect(), cells: Vec::new() });
        }
        cur.clear();
    };
    let outline = |l: &str| {
        let t = l.trim();
        let (head, _) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
        let core = head.trim_end_matches(['.', ')']);
        (head.ends_with('.') || head.ends_with(')'))
            && !core.is_empty()
            && core.len() <= 4
            && (core.chars().all(|c| c.is_ascii_digit() || c == '.') || (core.len() == 1 && core.chars().all(|c| c.is_ascii_alphabetic())))
    };
    for line in text.lines().take(400_000) {
        let l = line.trim();
        if l.is_empty() {
            flush(&mut cur, &cur_at, &mut doc);
            continue;
        }
        if l.starts_with("--- page") {
            page = l.trim_matches('-').trim().to_string();
            flush(&mut cur, &cur_at, &mut doc);
            continue;
        }
        // A section's page footer (`262726 - 2`).
        if l.len() <= 16 && l.contains(" - ") && l.chars().filter(char::is_ascii_digit).count() >= 6 {
            page = l.to_string();
            continue;
        }
        if line.contains("   ") && line.split("   ").filter(|c| !c.trim().is_empty()).count() >= 2 {
            table.push(line.split("   ").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect());
        } else if !table.is_empty() {
            table.push(vec![l.to_string()]);
        }
        if outline(l) || l.starts_with("PART ") || l.starts_with("SECTION ") {
            flush(&mut cur, &cur_at, &mut doc);
            cur_at = if page.is_empty() { name.to_string() } else { page.clone() };
        }
        if cur.is_empty() {
            cur_at = if page.is_empty() { name.to_string() } else { page.clone() };
        }
        if cur.ends_with('-') {
            cur.push_str(l);
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(l);
        }
        if cur.len() > MAX_PARA_CHARS {
            flush(&mut cur, &cur_at, &mut doc);
        }
    }
    flush(&mut cur, &cur_at, &mut doc);
    let rows = matrix_paras(&table, &format!("{name} matrix row"));
    doc.paras.extend(rows);
    doc.paras.truncate(MAX_PARAS);
    doc
}

// ─── Statements ─────────────────────────────────────────────────────────────

/// Where a statement was read, quoted.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cite {
    pub doc: String,
    pub at: String,
    pub text: String,
    pub tier: SourceTier,
}

impl Cite {
    pub(crate) fn words(&self) -> String {
        let at = if self.at.is_empty() || self.at == self.doc { String::new() } else { format!(" {}", self.at) };
        format!("{}{at}: \"{}\"", self.doc, self.text)
    }
    fn json(&self) -> Value {
        json!({"document": self.doc, "at": self.at, "text": self.text, "tier": self.tier.as_str()})
    }
}

/// What one sentence says about a class.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Says {
    /// The element itself, furnished and/or installed by a party (`explicit`: the party is named).
    Element { party: String, furnish: bool, install: bool, explicit: bool },
    /// Rough-in items a party does (`ours` when it is the electrical trade).
    Items { party: String, items: Vec<(String, String)>, explicit: bool },
    /// Items the documents say aren't needed for it.
    NotRequired { items: Vec<String> },
    /// The element is roughed in only (somebody else's).
    RoughInOnly,
    /// A deferred submittal, delegated design or design-build scope.
    DesignBuild,
    /// A party named beside it (`VERIFY … WITH LOW VOLTAGE VENDOR`).
    Mention { party: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Statement {
    pub class: String,
    pub says: Says,
    pub cite: Cite,
}

const VERBS: [&str; 10] = ["FURNISH", "PROVIDE", "SUPPLY", "INSTALL", "FURNISHES", "PROVIDES", "SUPPLIES", "INSTALLS", "MOUNT", "SET"];
const PARTICIPLES: [&str; 7] = ["FURNISHED", "PROVIDED", "SUPPLIED", "INSTALLED", "MOUNTED", "WIRED", "CONNECTED"];
/// Words that end the thing a verb acts on (`PROVIDE A CIRCUIT TO THE PANEL`: the circuit).
const STOPS: [&str; 18] =
    ["TO", "FOR", "AT", "ON", "IN", "FROM", "ABOVE", "BELOW", "NEAR", "ADJACENT", "PER", "UNLESS", "WHERE", "WHEN", "IF", "AS", "INTO", "WITHIN"];

fn is_verb(w: &str) -> bool {
    VERBS.contains(&w)
}

fn furnishes(w: &str) -> bool {
    matches!(w, "FURNISH" | "PROVIDE" | "SUPPLY" | "FURNISHES" | "PROVIDES" | "SUPPLIES" | "FURNISHED" | "PROVIDED" | "SUPPLIED")
}

fn installs(w: &str) -> bool {
    matches!(w, "INSTALL" | "PROVIDE" | "INSTALLS" | "PROVIDES" | "MOUNT" | "SET" | "INSTALLED" | "PROVIDED" | "MOUNTED")
}

/// The thing an object's words name first (up to a preposition; `WITH` keeps going: provided
/// with it, it is provided too).
fn head(object: &[String]) -> &[String] {
    let end = object.iter().position(|w| STOPS.contains(&w.as_str())).unwrap_or(object.len());
    object.get(..end).unwrap_or(object)
}

/// The classes a sentence's words name.
fn classes_named(r: &Rules, words: &[String]) -> Vec<String> {
    r.classes
        .iter()
        .filter(|c| c.words.iter().chain(c.system_words.iter()).any(|w| find(words, &toks(w)).is_some()))
        .map(|c| c.class.clone())
        .collect()
}

/// Does `words` name `class`'s element (or the whole system, where the class is the context)?
fn names_element(r: &Rules, class: &Class, words: &[String], context: bool) -> bool {
    class.words.iter().chain(class.system_words.iter()).any(|w| find(words, &toks(w)).is_some())
        || (context && r.whole_system.iter().any(|w| find(words, &toks(w)).is_some()))
}

/// The rough-in items an object names, each with the trade size before it (`3/4"`), and whether
/// the words call it electrical (`ELECTRICAL OUTLET BOXES`: ours whoever installs into it).
fn items_in(r: &Rules, words: &[String]) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = Vec::new();
    let found = spans(words, r.items.iter().flat_map(|(k, ws)| ws.iter().map(move |w| (w.as_str(), k.as_str()))));
    // An element's own name (`MANUAL FIRE-ALARM BOX`): no rough-in.
    let names_element = |s: usize| {
        r.classes.iter().flat_map(|c| c.words.iter()).any(|w| {
            let p = toks(w);
            s >= p.len() && words.get(s - p.len()..s).is_some_and(|x| x == p.as_slice())
        })
    };
    for (s, _, item) in found {
        if names_element(s) {
            continue;
        }
        let size = (s.saturating_sub(3)..s)
            .rev()
            .filter_map(|i| words.get(i))
            .find(|w| w.ends_with('"') && w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .cloned()
            .unwrap_or_default();
        let electrical = (s.saturating_sub(2)..s).filter_map(|i| words.get(i)).any(|w| w == "ELECTRICAL");
        match out.iter_mut().find(|(i, _, _)| *i == item) {
            Some(x) => {
                if x.1.is_empty() {
                    x.1 = size;
                }
                x.2 |= electrical;
            }
            None => out.push((item, size, electrical)),
        }
    }
    out
}

/// What each sentence of a paragraph says about the classes it (or its heading, or an earlier
/// sentence of it) names.
pub(crate) fn statements(doc: &Doc, para: &Para) -> Vec<Statement> {
    let r = rules();
    let mut out = Vec::new();
    let heading_classes = classes_named(r, &toks(&para.heading));
    if !para.cells.is_empty() {
        return matrix_statements(r, doc, para);
    }
    let mut context: Vec<String> = heading_classes.clone();
    let party_words = || r.parties.iter().flat_map(|(k, p)| p.words.iter().map(move |w| (w.as_str(), k.as_str())));
    for words in sentences(&para.text) {
        let parties = spans(&words, party_words());
        // The classes it names, outside its parties' names (`FIRE ALARM CONTRACTOR` names a
        // party, not the devices).
        let mut bare = words.clone();
        for (a, b, _) in &parties {
            for w in bare.iter_mut().take(*b).skip(*a) {
                w.clear();
            }
        }
        let named = classes_named(r, &bare);
        let from_context = named.is_empty();
        let classes: Vec<String> = if from_context { context.clone() } else { named.clone() };
        for c in &named {
            if !context.contains(c) {
                context.push(c.clone());
            }
        }
        if classes.is_empty() {
            continue;
        }
        let cite = Cite { doc: doc.name.clone(), at: para.at.clone(), text: words.join(" ").chars().take(QUOTE).collect(), tier: doc.tier };
        let mut says: Vec<(Option<String>, Says)> = Vec::new();
        // A table's cells read as one run of words (a shop drawing's legend) say nothing.
        if words.len() > MAX_SENTENCE {
            continue;
        }
        // (party, furnish, install, object, explicit)
        let mut acts: Vec<(String, bool, bool, Vec<String>, bool)> = Vec::new();
        for (s, e, party) in &parties {
            let (s, e) = (*s, *e);
            // `PARTY SHALL [NOT] FURNISH AND INSTALL …` / `PARTY PROVIDES …`.
            let mut j = e;
            let modal = words.get(j).is_some_and(|w| matches!(w.as_str(), "SHALL" | "WILL" | "MUST" | "TO"));
            if modal {
                j += 1;
                if words.get(j).is_some_and(|w| w == "BE") {
                    j = usize::MAX;
                }
            }
            let negative = words.get(j).is_some_and(|w| w == "NOT");
            if negative {
                j += 1;
            }
            let mut verbs: Vec<&str> = Vec::new();
            while let Some(w) = words.get(j) {
                if is_verb(w) {
                    verbs.push(w.as_str());
                } else if !matches!(w.as_str(), "AND" | "OR" | "/") || verbs.is_empty() {
                    break;
                }
                j += 1;
            }
            if !verbs.is_empty() && !negative && (modal || verbs.iter().any(|v| v.ends_with('S'))) {
                let object: Vec<String> = words.get(j..).map(<[String]>::to_vec).unwrap_or_default();
                acts.push((party.clone(), verbs.iter().any(|v| furnishes(v)), verbs.iter().any(|v| installs(v)), object, true));
                continue;
            }
            // `… FURNISHED AND INSTALLED BY [THE] PARTY` (`BY OTHERS`).
            let mut b = s;
            if b >= 1 && words.get(b - 1).is_some_and(|w| w == "THE") {
                b -= 1;
            }
            if b >= 1 && words.get(b - 1).is_some_and(|w| w == "BY") {
                let mut k = b - 1;
                let mut parts: Vec<&str> = Vec::new();
                while k >= 1 {
                    let Some(w) = words.get(k - 1) else { break };
                    if PARTICIPLES.contains(&w.as_str()) {
                        parts.push(w.as_str());
                    } else if !matches!(w.as_str(), "AND" | "OR" | "/" | "BE" | "IS" | "ARE" | "SHALL") {
                        break;
                    }
                    k -= 1;
                }
                // The subject: what comes before (the earlier `… BY X` of the same subject left out).
                let first_by = (0..k).find(|&i| words.get(i).is_some_and(|w| PARTICIPLES.contains(&w.as_str()))).unwrap_or(k);
                let subject: Vec<String> = words.get(..first_by.min(k)).map(<[String]>::to_vec).unwrap_or_default();
                let (f, i) = if parts.is_empty() { (true, true) } else { (parts.iter().any(|p| furnishes(p)), parts.iter().any(|p| installs(p))) };
                if !parts.is_empty() || party == "others" || party == "owner" {
                    acts.push((party.clone(), f, i, subject, true));
                    continue;
                }
            }
            // `… IS (ARE) THE RESPONSIBILITY OF [THE] [PROJECT] PARTY`.
            let back = s.saturating_sub(6);
            if let Some(rpos) =
                (back..s).find(|&i| words.get(i).is_some_and(|w| w == "RESPONSIBILITY") && words.get(i + 1).is_some_and(|w| w == "OF"))
            {
                let mut subj_end = rpos;
                while subj_end >= 1 && words.get(subj_end - 1).is_some_and(|w| matches!(w.as_str(), "THE" | "IS" | "ARE" | "SOLE")) {
                    subj_end -= 1;
                }
                let subject: Vec<String> = words.get(..subj_end).map(<[String]>::to_vec).unwrap_or_default();
                acts.push((party.clone(), true, true, subject, true));
                continue;
            }
            // `COORDINATE … WITH OWNER` names who to ask, not who does it.
            if party != &r.ours && party != "owner" && party != "others" {
                says.push((None, Says::Mention { party: party.clone() }));
            }
        }
        // An order with no one named: the document's trade's.
        if acts.is_empty()
            && let Some(trade) = &doc.trade
        {
            let start = if words.first().is_some_and(|w| w == "CONTRACTOR") && words.get(1).is_some_and(|w| w == "SHALL") { 2 } else { 0 };
            let mut j = start;
            let mut verbs: Vec<&str> = Vec::new();
            while let Some(w) = words.get(j) {
                if is_verb(w) && !w.ends_with('S') {
                    verbs.push(w.as_str());
                } else if !matches!(w.as_str(), "AND" | "OR" | "/") || verbs.is_empty() {
                    break;
                }
                j += 1;
            }
            if !verbs.is_empty() {
                let object: Vec<String> = words.get(j..).map(<[String]>::to_vec).unwrap_or_default();
                acts.push((trade.clone(), verbs.iter().any(|v| furnishes(v)), verbs.iter().any(|v| installs(v)), object, false));
            }
        }
        for (party, f, i, object, explicit) in acts {
            let head_words = head(&object);
            for cname in &classes {
                let Some(class) = r.class(cname) else { continue };
                // An order on our own trade's sheet with no one named gives only our rough-in:
                // the element is whoever's the documents name.
                let element_ok = explicit || party != r.ours;
                if element_ok
                    && names_element(
                        r,
                        class,
                        head_words,
                        from_context || !named.contains(cname) || class.system_words.iter().any(|w| find(head_words, &toks(w)).is_some()),
                    )
                {
                    says.push((Some(cname.clone()), Says::Element { party: party.clone(), furnish: f, install: i, explicit }));
                }
                // What is ordered (`PROVIDE A CIRCUIT TO THE PANEL`: the circuit), and anything the
                // words call electrical wherever it is (`… IN ELECTRICAL OUTLET BOXES`).
                let mut items = items_in(r, head_words);
                for x in items_in(r, &object).into_iter().filter(|x| x.2) {
                    if !items.iter().any(|i| i.0 == x.0) {
                        items.push(x);
                    }
                }
                if !items.is_empty() {
                    let mut ours_items = Vec::new();
                    let mut theirs = Vec::new();
                    for (it, size, electrical) in items {
                        if party == r.ours || electrical {
                            ours_items.push((it, size));
                        } else {
                            theirs.push((it, size));
                        }
                    }
                    if !ours_items.is_empty() {
                        says.push((Some(cname.clone()), Says::Items { party: r.ours.clone(), items: ours_items, explicit }));
                    }
                    if !theirs.is_empty() {
                        says.push((Some(cname.clone()), Says::Items { party: party.clone(), items: theirs, explicit }));
                    }
                }
            }
        }
        // `ROUGH-IN ONLY`.
        if r.rough_in_only.iter().any(|w| find(&words, &toks(w)).is_some()) {
            says.push((None, Says::RoughInOnly));
        }
        // `NO CONDUIT OR JUNCTION BOX SHALL BE REQUIRED`.
        let mut k = 0;
        while let Some(no) = find_from(&words, &["NO".to_string()], k) {
            let window: Vec<String> = words.get(no + 1..(no + 1 + REACH).min(words.len())).map(<[String]>::to_vec).unwrap_or_default();
            if let Some(req) = window.iter().position(|w| w == "REQUIRED" || w == "NEEDED") {
                let items: Vec<String> = items_in(r, window.get(..req).unwrap_or_default()).into_iter().map(|(i, _, _)| i).collect();
                if !items.is_empty() {
                    says.push((None, Says::NotRequired { items }));
                }
            }
            k = no + 1;
        }
        if r.design_build.iter().any(|w| find(&words, &toks(w)).is_some()) {
            says.push((None, Says::DesignBuild));
        }
        for (only, s) in says {
            for cname in classes.iter().filter(|c| only.as_ref().is_none_or(|o| o == *c)) {
                if out.len() < MAX_STATEMENTS {
                    out.push(Statement { class: cname.clone(), says: s.clone(), cite: cite.clone() });
                }
            }
        }
    }
    out
}

/// A matrix row's statements: its item's class furnished and installed by the parties under
/// those columns; a wiring, conduit, box or power column's party does those.
fn matrix_statements(r: &Rules, doc: &Doc, para: &Para) -> Vec<Statement> {
    let mut out = Vec::new();
    let Some(item) = para.cells.first() else { return out };
    let classes = classes_named(r, &toks(item));
    let cols: Vec<String> = para.heading.split(" | ").map(str::to_ascii_uppercase).collect();
    let cite = Cite { doc: doc.name.clone(), at: para.at.clone(), text: para.text.chars().take(QUOTE).collect(), tier: doc.tier };
    for (k, cell) in para.cells.iter().enumerate().skip(1) {
        let role = cols.get(k + cols.len().saturating_sub(para.cells.len())).cloned().unwrap_or_default();
        let Some(party) = r.party_named(cell) else { continue };
        for c in &classes {
            let s = if role.contains("FURNISH") || role.contains("PROVIDED") || role.contains("SUPPLIED") {
                Says::Element { party: party.clone(), furnish: true, install: false, explicit: true }
            } else if role.contains("INSTALL") || role.contains("MOUNT") {
                Says::Element { party: party.clone(), furnish: false, install: true, explicit: true }
            } else {
                let items: Vec<(String, String)> = items_in(r, &toks(&role)).into_iter().map(|(i, s, _)| (i, s)).collect();
                let items = if items.is_empty() && (role.contains("WIRED") || role.contains("ROUGH")) {
                    vec![("box".to_string(), String::new()), ("conduit".to_string(), String::new())]
                } else {
                    items
                };
                if items.is_empty() {
                    continue;
                }
                Says::Items { party: party.clone(), items, explicit: true }
            };
            out.push(Statement { class: c.clone(), says: s, cite: cite.clone() });
        }
    }
    out
}

// ─── Decisions ──────────────────────────────────────────────────────────────

/// One rough-in item of ours.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OurItem {
    pub item: String,
    pub size: String,
}

/// Who does a class's work.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Decision {
    pub class: String,
    pub label: String,
    pub furnish: String,
    pub install: String,
    /// `documents`, `answer` (a person's) or `default`.
    pub party_from: String,
    pub party_cites: Vec<Cite>,
    pub ours: Vec<OurItem>,
    pub ours_from: String,
    pub ours_cites: Vec<Cite>,
    pub design_build: Option<Cite>,
    /// Other parties other documents name for it.
    pub also: Vec<(String, Cite)>,
    /// Why the default holds where it does.
    pub reason: String,
}

impl Decision {
    /// Nothing of it is ours to furnish (it is somebody else's element).
    pub(crate) fn by_others(&self) -> bool {
        self.furnish != rules().ours
    }

    pub(crate) fn party_label(&self) -> String {
        let r = rules();
        if self.furnish == self.install {
            r.label(&self.furnish)
        } else {
            format!("furnished by {}, installed by {}", r.label(&self.furnish), r.label(&self.install))
        }
    }

    /// The source words of the party decision (or the default's reason).
    pub(crate) fn party_source(&self) -> String {
        match self.party_cites.first() {
            Some(c) => c.words(),
            None => format!("default: {}", self.reason),
        }
    }

    pub(crate) fn ours_source(&self) -> String {
        match self.ours_cites.first() {
            Some(c) => c.words(),
            None => format!("default: {}", self.reason),
        }
    }

    /// Our items for an element of `kind` (its fire alarm kind too): `power` only for the kinds
    /// that take a circuit of their own.
    pub(crate) fn ours_for(&self, kind: &str, fa_kind: &str) -> Vec<OurItem> {
        let power_kinds = rules().class(&self.class).map(|c| c.power_kinds.clone()).unwrap_or_default();
        let k = kind.replace('_', " ");
        self.ours
            .iter()
            .filter(|i| i.item != "power" || power_kinds.iter().any(|p| p.eq_ignore_ascii_case(&k) || p.eq_ignore_ascii_case(fa_kind)))
            .cloned()
            .collect()
    }

    pub(crate) fn to_json(&self) -> Value {
        let r = rules();
        json!({
            "class": self.class, "label": self.label,
            "furnish": self.furnish, "install": self.install, "party": self.party_label(),
            "by_others": self.by_others(),
            "party_from": self.party_from, "party_sources": self.party_cites.iter().map(Cite::json).collect::<Vec<_>>(),
            "ours": self.ours.iter().map(|i| json!({"item": i.item, "size": i.size, "bought_as": r.item_name(&i.item)})).collect::<Vec<_>>(),
            "ours_from": self.ours_from, "ours_sources": self.ours_cites.iter().map(Cite::json).collect::<Vec<_>>(),
            "design_build": self.design_build.as_ref().map(Cite::json),
            "also": self.also.iter().map(|(p, c)| json!({"party": p, "source": c.json()})).collect::<Vec<_>>(),
            "reason": self.reason,
        })
    }
}

/// Precedence of a statement: a named party before an order of a document's trade, a general
/// note before a keynote (it says what is done where its marker is, not for every element),
/// then the document's tier (an approved submittal, a drawing, the specification).
fn rank(explicit: bool, c: &Cite) -> (usize, usize, usize) {
    (usize::from(!explicit), usize::from(keyed(c)), c.tier as usize)
}

/// Is it a keynote's (a note of a `KEYNOTES` list)?
fn keyed(c: &Cite) -> bool {
    let u = c.at.to_ascii_uppercase();
    u.contains("KEYNOTE") || u.contains("KEYED NOTE")
}

/// The default decision of a class.
fn default_of(c: &Class) -> Decision {
    Decision {
        class: c.class.clone(),
        label: c.label.clone(),
        furnish: c.party.clone(),
        install: c.party.clone(),
        party_from: "default".into(),
        ours: c.default_ours.iter().map(|i| OurItem { item: i.clone(), size: String::new() }).collect(),
        ours_from: "default".into(),
        reason: c.default_reason.clone(),
        ..Decision::default()
    }
}

/// Each class's decision from what the documents say.
pub(crate) fn decide(stmts: &[Statement]) -> Vec<Decision> {
    let r = rules();
    let mut out = Vec::new();
    for c in &r.classes {
        let mine: Vec<&Statement> = stmts.iter().filter(|s| s.class == c.class).collect();
        let mut d = default_of(c);
        // The parties: the best-ranked statement for each.
        let pick = |want: fn(bool, bool) -> bool| {
            mine.iter()
                .filter_map(|s| match &s.says {
                    Says::Element { party, furnish, install, explicit } if want(*furnish, *install) => {
                        Some((rank(*explicit, &s.cite), party.clone(), s.cite.clone()))
                    }
                    _ => None,
                })
                .min_by(|a, b| a.0.cmp(&b.0))
        };
        let mentioned = || {
            let mut n: BTreeMap<&str, usize> = BTreeMap::new();
            for s in &mine {
                if let Says::Mention { party } = &s.says
                    && party != &r.ours
                    && party != "others"
                {
                    *n.entry(party.as_str()).or_default() += 1;
                }
            }
            n.into_iter().max_by_key(|(_, k)| *k).map(|(p, _)| p.to_string())
        };
        let mention_cite = |p: &str| mine.iter().find(|s| matches!(&s.says, Says::Mention { party } if party == p)).map(|s| s.cite.clone());
        let named = |p: String| if p == "others" { mentioned().unwrap_or_else(|| c.party.clone()) } else { p };
        let furnish = pick(|f, _| f);
        let install = pick(|_, i| i);
        let rough_only = mine.iter().find(|s| matches!(s.says, Says::RoughInOnly));
        match (&furnish, &install) {
            (None, None) => {
                if let Some(ro) = rough_only {
                    let p = mentioned().unwrap_or_else(|| c.party.clone());
                    d.furnish = p.clone();
                    d.install = p.clone();
                    d.party_from = "documents".into();
                    d.party_cites.push(ro.cite.clone());
                    if let Some(m) = mention_cite(&p) {
                        d.party_cites.push(m);
                    }
                }
            }
            _ => {
                let (f, i) = (furnish.clone().or_else(|| install.clone()), install.clone().or_else(|| furnish.clone()));
                if let (Some(f), Some(i)) = (f, i) {
                    d.furnish = named(f.1.clone());
                    d.install = named(i.1.clone());
                    d.party_from = "documents".into();
                    d.party_cites.push(f.2.clone());
                    if i.2 != f.2 {
                        d.party_cites.push(i.2.clone());
                    }
                    if (f.1 == "others" || i.1 == "others")
                        && let Some(m) = mention_cite(&d.furnish)
                    {
                        d.party_cites.push(m);
                    }
                }
            }
        }
        // Other parties other statements name for the element.
        for s in &mine {
            if let Says::Element { party, .. } = &s.says {
                let p = named(party.clone());
                if p != d.furnish && p != d.install && d.also.len() < 8 && !d.also.iter().any(|(x, _)| *x == p) {
                    d.also.push((p, s.cite.clone()));
                }
            }
        }
        // What of its rough-in is ours: each item by its best statement.
        let mut best: BTreeMap<String, ((usize, usize, usize), bool, String, Cite)> = BTreeMap::new();
        for s in &mine {
            let mut put = |item: &str, ours: bool, size: &str, k: (usize, usize, usize)| {
                let better = best.get(item).is_none_or(|b| k < b.0);
                if better {
                    best.insert(item.to_string(), (k, ours, size.to_string(), s.cite.clone()));
                }
            };
            match &s.says {
                Says::Items { party, items, explicit } => {
                    for (it, size) in items {
                        put(it, party == &r.ours, size, rank(*explicit, &s.cite));
                    }
                }
                // A document saying it isn't needed outranks one ordering it at the same tier.
                Says::NotRequired { items } => {
                    for it in items {
                        put(it, false, "", (0, usize::from(keyed(&s.cite)), s.cite.tier as usize));
                    }
                }
                _ => {}
            }
        }
        if !best.is_empty() {
            d.ours_from = "documents".into();
            d.ours = best.iter().filter(|(_, b)| b.1).map(|(k, b)| OurItem { item: k.clone(), size: b.2.clone() }).collect();
            // The deciding words first (the best-ranked).
            let mut ranked: Vec<&((usize, usize, usize), bool, String, Cite)> = best.values().collect();
            ranked.sort_by_key(|b| b.0);
            let mut cites: Vec<Cite> = Vec::new();
            for b in ranked {
                if !cites.contains(&b.3) {
                    cites.push(b.3.clone());
                }
            }
            d.ours_cites = cites;
        }
        d.design_build = mine.iter().find(|s| matches!(s.says, Says::DesignBuild)).map(|s| s.cite.clone());
        // Our own element: nothing of it is anyone else's.
        out.push(d);
    }
    out
}

/// What the documents say, read: their statements and the decisions they make.
pub(crate) fn read_docs(docs: &[Doc]) -> (Vec<Statement>, Vec<Decision>) {
    let mut stmts = Vec::new();
    for d in docs {
        for p in &d.paras {
            if stmts.len() >= MAX_STATEMENTS {
                break;
            }
            stmts.extend(statements(d, p));
        }
    }
    stmts.truncate(MAX_STATEMENTS);
    let decisions = decide(&stmts);
    (stmts, decisions)
}

// ─── Kept in the project rules ──────────────────────────────────────────────

fn key(class: &str, aspect: &str) -> String {
    format!("scope.{}.{aspect}", class.replace(' ', "_"))
}

/// The tier a decision's sources give it: its deciding source's (the first).
fn tier_of(cites: &[Cite]) -> SourceTier {
    cites.first().map_or(SourceTier::BuiltIn, |c| c.tier)
}

/// The tiers a reading of the documents keeps a decision at: a new reading replaces them all
/// (a person's answer, an RFI or the field stays).
const READ_TIERS: [SourceTier; 4] = [SourceTier::Submittal, SourceTier::DrawingRevision, SourceTier::Spec, SourceTier::BuiltIn];

fn replace_reading(d: &mut cadaddy_doc::Drawing, key: &str, value: RuleValue) {
    for t in READ_TIERS {
        d.project_rules.remove_tier(RuleDomain::Custom, key, t);
    }
    d.project_rules.set(RuleDomain::Custom, key, value, "read from the documents (bim.scope)");
}

/// Keep the documents' decisions in the project rules (defaults aren't kept: they are what holds
/// with nothing kept). Returns the classes recorded.
pub(crate) fn record(d: &mut cadaddy_doc::Drawing, decisions: &[Decision], by: &str) -> usize {
    let mut n = 0;
    for x in decisions {
        if x.party_from == "documents" {
            let v = json!({"furnish": x.furnish, "install": x.install, "design_build": x.design_build.as_ref().map(Cite::words),
                           "also": x.also.iter().map(|(p, c)| json!({"party": p, "source": c.words()})).collect::<Vec<_>>()});
            let src = x.party_cites.iter().map(Cite::words).collect::<Vec<_>>().join(" | ");
            replace_reading(
                d,
                &key(&x.class, "party"),
                RuleValue::new(v.to_string())
                    .with_source(src.chars().take(1_200).collect::<String>())
                    .with_tier(tier_of(&x.party_cites))
                    .with_set_by(by),
            );
            n += 1;
        }
        if x.ours_from == "documents" {
            let v = json!(x.ours.iter().map(|i| json!({"item": i.item, "size": i.size})).collect::<Vec<_>>());
            let src = x.ours_cites.iter().map(Cite::words).collect::<Vec<_>>().join(" | ");
            replace_reading(
                d,
                &key(&x.class, "ours"),
                RuleValue::new(v.to_string())
                    .with_source(src.chars().take(1_200).collect::<String>())
                    .with_tier(tier_of(&x.ours_cites))
                    .with_set_by(by),
            );
        }
    }
    n
}

/// The source text kept with a rule into a citation.
fn cite_of_rule(source: &str, tier: SourceTier) -> Cite {
    let first = source.split(" | ").next().unwrap_or_default();
    let (doc, text) = first.split_once(": ").unwrap_or((first, ""));
    Cite { doc: doc.to_string(), at: String::new(), text: text.trim_matches('"').to_string(), tier }
}

/// Each class's decision as the drawing keeps it (the default where nothing is kept).
pub(crate) fn decisions(d: &cadaddy_doc::Drawing) -> Vec<Decision> {
    let r = rules();
    r.classes
        .iter()
        .map(|c| {
            let mut x = default_of(c);
            if let Some(v) = d.project_rules.resolve(RuleDomain::Custom, &key(&c.class, "party"))
                && let Ok(j) = serde_json::from_str::<Value>(&v.value)
            {
                let p = |k: &str| j[k].as_str().map(str::to_string).filter(|s| !s.is_empty());
                if let (Some(f), Some(i)) = (p("furnish"), p("install")) {
                    x.furnish = f;
                    x.install = i;
                    x.party_from = if v.tier == SourceTier::ProjectRule { "answer".into() } else { "documents".into() };
                    x.party_cites = v.source.split(" | ").take(4).map(|s| cite_of_rule(s, v.tier)).collect();
                    x.design_build = p("design_build").map(|s| cite_of_rule(&s, v.tier));
                }
            }
            if let Some(v) = d.project_rules.resolve(RuleDomain::Custom, &key(&c.class, "ours"))
                && let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&v.value)
            {
                x.ours = a
                    .iter()
                    .take(16)
                    .filter_map(|i| {
                        let item = i["item"].as_str().or_else(|| i.as_str())?;
                        Some(OurItem { item: item.to_string(), size: i["size"].as_str().unwrap_or_default().to_string() })
                    })
                    .collect();
                x.ours_from = if v.tier == SourceTier::ProjectRule { "answer".into() } else { "documents".into() };
                x.ours_cites = v.source.split(" | ").take(4).map(|s| cite_of_rule(s, v.tier)).collect();
            }
            x
        })
        .collect()
}

/// A person's decision for a class (a review answer, `bim.scope {set}`): its party (`furnish`
/// and `install`) and/or what of it is ours, at the project-rule tier.
pub(crate) fn answer(d: &mut cadaddy_doc::Drawing, class: &str, answer: &Value, source: &str) -> std::result::Result<(), String> {
    let r = rules();
    let Some(c) = r.class(class) else {
        return Err(format!("no class `{class}` (one of {})", r.classes.iter().map(|c| c.class.as_str()).collect::<Vec<_>>().join(", ")));
    };
    let party = |v: &Value| v.as_str().and_then(|s| r.party_named(s));
    let (furnish, install) = match answer {
        Value::String(s) if s.trim().eq_ignore_ascii_case("ours") => (Some(r.ours.clone()), Some(r.ours.clone())),
        Value::String(_) => (party(answer), party(answer)),
        Value::Object(o) => {
            let both = o.get("party").and_then(party);
            (o.get("furnish").and_then(party).or_else(|| both.clone()), o.get("install").and_then(party).or(both))
        }
        _ => (None, None),
    };
    let ours: Option<Vec<OurItem>> = match answer.get("ours") {
        Some(Value::Array(a)) => Some(
            a.iter()
                .take(16)
                .filter_map(|i| i.as_str().map(|s| (s, "")).or_else(|| Some((i["item"].as_str()?, i["size"].as_str().unwrap_or_default()))))
                .filter(|(i, _)| r.items.contains_key(*i))
                .map(|(i, s)| OurItem { item: i.to_string(), size: s.to_string() })
                .collect(),
        ),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("none") => Some(Vec::new()),
        _ => None,
    };
    if furnish.is_none() && install.is_none() && ours.is_none() {
        return Err(format!(
            "answer with a party ({}), {{furnish, install}}, or {{ours: [{}] | \"none\"}}",
            r.parties.keys().cloned().collect::<Vec<_>>().join(", "),
            r.items.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if furnish.is_some() || install.is_some() {
        let f = furnish.or_else(|| install.clone()).unwrap_or_else(|| c.party.clone());
        let i = install.unwrap_or_else(|| f.clone());
        d.project_rules.set(
            RuleDomain::Custom,
            &key(&c.class, "party"),
            RuleValue::new(json!({"furnish": f, "install": i}).to_string())
                .with_source(source)
                .with_tier(SourceTier::ProjectRule)
                .with_set_by("user"),
            "answered",
        );
    }
    if let Some(o) = ours {
        d.project_rules.set(
            RuleDomain::Custom,
            &key(&c.class, "ours"),
            RuleValue::new(json!(o.iter().map(|i| json!({"item": i.item, "size": i.size})).collect::<Vec<_>>()).to_string())
                .with_source(source)
                .with_tier(SourceTier::ProjectRule)
                .with_set_by("user"),
            "answered",
        );
    }
    Ok(())
}

// ─── The model's elements ───────────────────────────────────────────────────

/// An element tag's class (if another trade may own it) and its fire alarm kind.
pub(crate) fn tag_class<'a>(r: &'a Rules, t: &cadaddy_walk3d::tags::ElementTag) -> Option<&'a Class> {
    let what = t.fields.get("device_type").map_or("", |f| f.value.as_str());
    r.class_of(&t.kind, what)
}

/// The questions to ask: each class the model holds elements of whose party or rough-in no
/// document settled. (class, aspect `party`|`ours`, elements, the default as words.)
pub(crate) fn questions(d: &cadaddy_doc::Drawing, sp: &cadaddy_walk3d::BuildingSpec) -> Vec<(String, String, usize, String)> {
    let r = rules();
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for t in &sp.tags {
        if let Some(c) = tag_class(r, t) {
            *count.entry(c.class.clone()).or_default() += 1;
        }
    }
    for e in &sp.mep.equipment {
        if let Some(c) = r.class_of(&e.kind, &e.title) {
            *count.entry(c.class.clone()).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for x in decisions(d) {
        let Some(&n) = count.get(&x.class) else { continue };
        if x.party_from == "default" {
            out.push((x.class.clone(), "party".to_string(), n, format!("default: {} ({})", x.party_label(), x.reason)));
        }
        if x.ours_from == "default" && x.by_others() {
            let items: Vec<String> = x.ours.iter().map(|i| r.item_name(&i.item)).collect();
            out.push((
                x.class.clone(),
                "ours".to_string(),
                n,
                format!("default: ours is {} ({})", if items.is_empty() { "nothing".to_string() } else { items.join(", ") }, x.reason),
            ));
        }
    }
    out
}

/// The fire alarm devices given their kinds: each by its legend row (its mark's words in the
/// sheets' legends), else its own name; a mark two rows give (a detector and a horn/strobe drawn
/// with one letter) by its mounting, else left for a question. Returns (resolved,
/// unresolved marks with the kinds they might be).
pub(crate) fn fa_kinds(d: &cadaddy_doc::Drawing, sp: &mut cadaddy_walk3d::BuildingSpec) -> (usize, BTreeMap<String, Vec<String>>) {
    let r = rules();
    let Some(fa) = r.classes.iter().find(|c| c.party == "fire alarm") else { return (0, BTreeMap::new()) };
    let ours = |t: &cadaddy_walk3d::tags::ElementTag| tag_class(r, t).is_some_and(|c| c.class == fa.class);
    let marks: BTreeSet<String> = sp
        .tags
        .iter()
        .filter(|t| ours(t))
        .filter_map(|t| t.fields.get("type").map(|f| f.value.trim().to_ascii_uppercase()))
        .filter(|m| !m.is_empty())
        .collect();
    let docs = super::validate_height::DocHeights::of(d, sp, &marks);
    let rows: BTreeMap<String, Vec<String>> =
        marks.iter().map(|m| (m.clone(), docs.legend_words(m).into_iter().map(str::to_string).take(8).collect())).collect();
    let mut resolved = 0;
    let mut open: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in sp.tags.iter_mut().filter(|t| ours(t)) {
        let mark = t.fields.get("type").map(|f| f.value.trim().to_ascii_uppercase()).unwrap_or_default();
        let mount = t.fields.get("mount").map(|f| f.value.to_ascii_uppercase()).unwrap_or_default();
        // Its own words: its name, what its legend row said it is (a symbol with no mark).
        let own = format!(
            "{} {} {}",
            t.fields.get("fixture").map_or("", |f| f.value.as_str()),
            t.fields.get("kind_source").map_or("", |f| f.value.as_str()),
            t.kind.replace('_', " ")
        );
        let mut cands: Vec<(String, String)> =
            rows.get(&mark).into_iter().flatten().filter_map(|w| r.fa_kind_of(w).map(|k| (k, w.clone()))).collect();
        if cands.is_empty()
            && let Some(k) = r.fa_kind_of(&own)
        {
            cands.push((k, own.trim().to_string()));
        }
        let kinds: BTreeSet<&str> = cands.iter().map(|(k, _)| k.as_str()).collect();
        let pick = if kinds.len() == 1 {
            cands.first().cloned().map(|(k, w)| (k, format!("legend row of mark {mark}: {w}")))
        } else if kinds.len() > 1 && !mount.is_empty() {
            // Its mounting: the rows that may be mounted so.
            let fits: Vec<&(String, String)> = cands
                .iter()
                .filter(|(_, w)| {
                    let u = w.to_ascii_uppercase();
                    if mount.contains("CEIL") { u.contains("CEIL") } else { u.contains(&mount) }
                })
                .collect();
            let fit_kinds: BTreeSet<&str> = fits.iter().map(|(k, _)| k.as_str()).collect();
            (fit_kinds.len() == 1)
                .then(|| fits.first().map(|(k, w)| (k.clone(), format!("legend row of mark {mark} mounted {}: {w}", mount.to_ascii_lowercase()))))
                .flatten()
        } else {
            None
        };
        match pick {
            Some((k, src)) => {
                t.set("fa_kind", k, "legend");
                t.set("fa_kind_from", src, "legend");
                resolved += 1;
            }
            None => {
                let e = open.entry(mark.clone()).or_default();
                for k in kinds {
                    if !e.iter().any(|x| x == k) {
                        e.push(k.to_string());
                    }
                }
            }
        }
    }
    (resolved, open)
}

/// A fire alarm panel's room as the documents name it (`THE FIRE ALARM CONTROL PANEL … LOCATED …
/// IN ELECTRICAL ROOM 610 … WITH AN ANNUNCIATOR IN LOBBY 605`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanelPlace {
    pub kind: String,
    pub tag: String,
    pub room: String,
    pub words: String,
    /// The floor the words name (`1ST FL`: 1), if any.
    pub floor: Option<u32>,
    pub cite: Cite,
}

/// `1ST`, `FIRST`, `2ND`… → the floor number.
fn floor_word(w: &str) -> Option<u32> {
    let ordinals = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH"];
    if let Some(i) = ordinals.iter().position(|o| *o == w) {
        return u32::try_from(i + 1).ok();
    }
    let n: String = w.chars().take_while(char::is_ascii_digit).collect();
    let rest = w.get(n.len()..).unwrap_or_default();
    (matches!(rest, "ST" | "ND" | "RD" | "TH") && n.len() <= 2).then(|| n.parse().ok()).flatten()
}

/// Where the documents put each fire alarm panel: a panel's words, then `IN` (or `AT`) a room's
/// words and number, before the next panel's words.
pub(crate) fn panel_places(docs: &[Doc]) -> Vec<PanelPlace> {
    let r = rules();
    let mut out: Vec<PanelPlace> = Vec::new();
    for d in docs {
        for p in &d.paras {
            for words in sentences(&p.text) {
                let found = spans(&words, r.panels.iter().flat_map(|pr| pr.words.iter().map(move |w| (w.as_str(), pr.kind.as_str()))));
                for (k, (_, e, kind)) in found.iter().enumerate() {
                    let stop = found.get(k + 1).map_or(words.len(), |n| n.0);
                    let Some(rule) = r.panels.iter().find(|pr| pr.kind == *kind) else { continue };
                    let floor = words.get(*e..stop).and_then(|w| w.iter().find_map(|x| floor_word(x)));
                    let Some(at) = (*e..stop).find(|&i| words.get(i).is_some_and(|w| w == "IN" || w == "AT")) else { continue };
                    // The room: its words, then a number (digits in it), a few words on.
                    let Some(num) = (at + 1..(at + 6).min(stop)).find(|&i| {
                        words
                            .get(i)
                            .is_some_and(|w| w.len() <= 6 && w.chars().any(|c| c.is_ascii_digit()) && floor_word(w).is_none() && !w.ends_with('"'))
                    }) else {
                        continue;
                    };
                    let room = words.get(num).cloned().unwrap_or_default();
                    let room_words = words.get(at + 1..num).map(|w| w.join(" ")).unwrap_or_default();
                    let cite = Cite { doc: d.name.clone(), at: p.at.clone(), text: words.join(" ").chars().take(QUOTE).collect(), tier: d.tier };
                    let better = out.iter().position(|o| o.kind == *kind);
                    let place = PanelPlace { kind: kind.clone(), tag: rule.tag.clone(), room, words: room_words, floor, cite };
                    match better {
                        Some(i) if out.get(i).is_some_and(|o| (place.cite.tier as usize) < (o.cite.tier as usize)) => {
                            if let Some(o) = out.get_mut(i) {
                                *o = place;
                            }
                        }
                        Some(_) => {}
                        None => out.push(place),
                    }
                }
            }
        }
    }
    out
}

/// The fire alarm panels the documents locate, laid along a wall of their rooms (proposed), as
/// equipment of the spec; those already there are left. Returns the tags placed and those not
/// (with why).
pub(crate) fn place_panels(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    places: &[PanelPlace],
    rooms: &[super::gearbuild::RoomBox],
) -> (Vec<String>, Vec<(String, String)>) {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut placed = Vec::new();
    let mut not = Vec::new();
    for p in places {
        if sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(&p.tag)) {
            continue;
        }
        let Some(rule) = r.panels.iter().find(|x| x.kind == p.kind) else { continue };
        let level_digit = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
        let room = rooms.iter().filter(|x| x.number.eq_ignore_ascii_case(&p.room)).find(|x| {
            p.floor.is_none_or(|f| level_digit(&x.level) == f.to_string())
                || rooms.iter().filter(|y| y.number.eq_ignore_ascii_case(&p.room)).count() == 1
        });
        let Some(room) = room else {
            not.push((p.tag.clone(), format!("room {} ({}) is not in the model", p.room, p.cite.words())));
            continue;
        };
        let [w, dp, h] = rule.size_in;
        let at = super::gearbuild::place_along_walls(&room.outline, &[(w / 12.0 * upf, dp / 12.0 * upf)], w / 12.0 * upf, 3.0 * upf, 0.5 * upf);
        let Some(Some(at)) = at.into_iter().next() else {
            not.push((p.tag.clone(), format!("no wall of room {} holds it", p.room)));
            continue;
        };
        let mut properties = BTreeMap::new();
        properties.insert("FA_KIND".to_string(), p.kind.clone());
        properties.insert("PLACEMENT".to_string(), format!("proposed: along a wall of room {} ({}), where the documents put it", p.room, p.words));
        let mut sources = BTreeMap::new();
        sources.insert("position".to_string(), p.cite.words());
        sources.insert("size".to_string(), r.panel_source.clone());
        sources.insert("status".to_string(), "proposed".to_string());
        sp.mep.equipment.push(cadaddy_walk3d::MepEquipment {
            tag: p.tag.clone(),
            kind: if p.kind == "annunciator" { "annunciator".into() } else { "fire alarm panel".into() },
            level: room.level.clone(),
            min: at.min,
            max: at.max,
            bottom_ft: ((rule.top_in - h) / 12.0).max(0.0),
            height_ft: h / 12.0,
            title: p.kind.to_ascii_uppercase(),
            properties,
            sources,
            confidence: 0.5,
            ..cadaddy_walk3d::MepEquipment::default()
        });
        placed.push(p.tag.clone());
    }
    (placed, not)
}

// ─── Documents given to the command ─────────────────────────────────────────

/// The model's own sheets as documents (their text, sheet by sheet).
pub(crate) fn model_docs(d: &cadaddy_doc::Drawing, sp: &cadaddy_walk3d::BuildingSpec) -> Vec<Doc> {
    let all = cadaddy_mepfix::sheet_facts::texts(d);
    let mut by: BTreeMap<String, Vec<TextItem>> = BTreeMap::new();
    for t in all {
        let sheet = sp.sheets.iter().find(|s| {
            s.rect.is_some_and(|r| t.at.x >= r[0].min(r[2]) && t.at.x <= r[0].max(r[2]) && t.at.y >= r[1].min(r[3]) && t.at.y <= r[1].max(r[3]))
        });
        let name = sheet.map_or_else(|| "the drawing".to_string(), |s| s.number.split('/').next().unwrap_or(&s.number).to_string());
        by.entry(name).or_default().push(t);
    }
    by.into_iter().map(|(n, texts)| sheet_doc(&n, &texts)).collect()
}

/// A specification section file's document: its section number and title, its division's trade.
pub(crate) fn spec_doc(file: &str, text: &str) -> Doc {
    let r = rules();
    let stem = std::path::Path::new(file).file_stem().map_or_else(|| file.to_string(), |n| n.to_string_lossy().to_string());
    let header = text.lines().take(200).find_map(|l| l.trim().strip_prefix("SECTION ").map(str::to_string));
    let name = header.map_or_else(|| stem.clone(), |h| h.trim().to_string());
    let trade = r.division_party(&name).or_else(|| r.division_party(&stem));
    text_doc(&name, SourceTier::Spec, trade, text)
}

/// A submittal file's document: its trade by the section number in its name (`#283111-2`), else
/// its words (`Fire Alarm`).
pub(crate) fn submittal_doc(file: &str, text: &str) -> Doc {
    let r = rules();
    let stem = std::path::Path::new(file).file_stem().map_or_else(|| file.to_string(), |n| n.to_string_lossy().to_string());
    let section: String = stem.split('#').nth(1).map(|s| s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect()).unwrap_or_default();
    let trade = r.division_party(&section).or_else(|| {
        let t = toks(&stem);
        r.classes.iter().find(|c| c.words.iter().chain(c.system_words.iter()).any(|w| find(&t, &toks(w)).is_some())).map(|c| c.party.clone())
    });
    text_doc(&stem, SourceTier::Submittal, trade, text)
}

/// Is a submittal file one whose notes may say who does a class's work (by its section's
/// division, or a class's words in its name)?
pub(crate) fn submittal_wanted(file: &str) -> bool {
    let r = rules();
    let stem = std::path::Path::new(file).file_stem().map_or_else(|| file.to_string(), |n| n.to_string_lossy().to_string());
    let section: String = stem.split('#').nth(1).map(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect()).unwrap_or_default();
    let by_section = r.division_party(&section).is_some_and(|p| p != r.ours);
    let t = toks(&stem);
    by_section || r.classes.iter().any(|c| c.words.iter().chain(c.system_words.iter()).any(|w| w.len() > 3 && find(&t, &toks(w)).is_some()))
}

/// A file's text (a PDF's, else as text).
fn file_text(f: &str) -> std::result::Result<String, String> {
    let b = crate::readfile::read(f).map_err(|e| e.to_string())?;
    if f.to_ascii_lowercase().ends_with(".pdf") {
        super::spec_import::pdf_text(&b, f).map_err(|e| e.to_string())
    } else {
        String::from_utf8(b).map_err(|e| e.to_string())
    }
}

/// The specification sections a scope reading wants (its divisions' files), from paths or
/// folders.
pub(crate) fn spec_paths(given: &[String]) -> Vec<String> {
    let r = rules();
    let mut wanted: Vec<String> = r.divisions.iter().map(|d| d.prefix.get(..2).unwrap_or(&d.prefix).to_string()).collect();
    wanted.push("01".into());
    wanted.sort();
    wanted.dedup();
    super::products::spec_files_in(given, &wanted)
}

/// Read the specification and submittal files into documents (`unread`: those that couldn't be).
pub(crate) fn file_docs(specs: &[String], submittals: &[String], unread: &mut Vec<Value>) -> Vec<Doc> {
    let mut docs = Vec::new();
    for f in spec_paths(specs).into_iter().take(MAX_FILES) {
        match file_text(&f) {
            Ok(t) => docs.push(spec_doc(&f, &t)),
            Err(e) => unread.push(json!({"file": f, "error": e})),
        }
    }
    for f in super::submittals::pdf_files(submittals).into_iter().filter(|f| submittal_wanted(f)).take(MAX_FILES) {
        match file_text(&f) {
            Ok(t) => docs.push(submittal_doc(&f, &t)),
            Err(e) => unread.push(json!({"file": f, "error": e})),
        }
    }
    docs
}

/// The rooms as built (for placing panels in them).
pub(crate) fn room_boxes(d: &cadaddy_doc::Drawing) -> Vec<super::gearbuild::RoomBox> {
    let offsets: BTreeMap<cadaddy_doc::Handle, cadaddy_geom::Vec2> =
        d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("room")).map(|e| (e.handle, super::bspec::stacked_by(e))).collect();
    super::bspec::room_elements(d)
        .into_iter()
        .map(|r| super::gearbuild::RoomBox {
            offset: offsets.get(&r.handle).copied().unwrap_or(cadaddy_geom::Vec2::ZERO),
            level: r.level,
            number: r.number.unwrap_or_default(),
            name: r.name,
            outline: r.outline,
        })
        .collect()
}

/// Build the fire alarm panels just placed, moved as the build moved their levels.
pub(crate) fn build_panels(
    d: &mut cadaddy_doc::Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    tags: &[String],
    rooms: &[super::gearbuild::RoomBox],
) -> usize {
    let only: BTreeSet<String> = tags.iter().map(|t| t.to_ascii_uppercase()).collect();
    if only.is_empty() {
        return 0;
    }
    let built = super::mep::build_equipment(d, sp, Some(&only));
    for h in &built {
        let level = d.model.iter().find(|e| e.handle == *h).and_then(|e| super::bspec::made_level(e));
        let off = level.and_then(|l| rooms.iter().find(|r| r.level.eq_ignore_ascii_case(&l)).map(|r| r.offset)).unwrap_or(cadaddy_geom::Vec2::ZERO);
        if off.len() > 1e-9 {
            let m = cadaddy_geom::Mat3::translate(off);
            let _ = d.modify_entity(*h, |e| {
                e.kind.transform(&m);
                e.common.xdata = super::bspec::with_stacked(&e.common.xdata, off).into();
            });
        }
    }
    built.len()
}

/// What the scope reading did, for a job's report and the command's answer.
pub(crate) fn summary(decisions: &[Decision], sp: &cadaddy_walk3d::BuildingSpec) -> Value {
    let r = rules();
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for t in &sp.tags {
        if let Some(c) = tag_class(r, t) {
            *count.entry(c.class.clone()).or_default() += 1;
        }
    }
    json!(decisions
        .iter()
        .map(|x| json!({"class": x.class, "elements": count.get(&x.class).copied().unwrap_or(0), "party": x.party_label(), "party_from": x.party_from,
                        "ours": x.ours.iter().map(|i| r.item_name(&i.item)).collect::<Vec<_>>(), "ours_from": x.ours_from}))
        .collect::<Vec<_>>())
}

// ─── bim.scope ──────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Scope by Trade", run)
        .params("{specs?: [path] (specification section PDFs or text, or folders of them: Divisions 01, 21-28 are read), submittals?: [path] (files or folders: those of another trade's section or naming a class), text?: string | [{document, text, tier?: drawing_revision|spec|submittal, trade?: party}] (notes given as text), pdfs?: [path] (drawing PDFs: each page's notes), set?: [{class, party? | furnish?, install?, ours?: [box|ring|conduit|pull string|sleeve|power] | \"none\", reason}] (a person's decision), sheets?: bool (read the drawing's own sheets; default true), kinds?: bool (give the fire alarm devices their kinds from the legends; default true), panels?: bool (place the fire alarm panels the documents locate; default true), explain?: bool (list each statement read), explain_documents?: [name] (and those documents' paragraphs)} → {decisions: [{class, label, furnish, install, party, by_others, party_from: documents|answer|default, party_sources: [{document, at, text, tier}], ours: [{item, size, bought_as}], ours_from, ours_sources, design_build?, also: [{party, source}], reason}], elements: [{class, elements, party, party_from, ours, ours_from}], statements: n, documents: n, fire_alarm: {resolved, unresolved: {mark: [kinds]}}, panels: {placed, not_placed}, questions: [{class, aspect, elements, default}], read?: [{class, says, source}], paragraphs?: [{document, at, heading, text}], unread}: who furnishes and installs each class of element another trade may own (thermostats, fire alarm devices, low voltage devices) and what of its rough-in is ours, read from the notes, specifications, submittals and responsibility matrices with the words that say so; kept in the project rules (scope.<class>.party|ours) at the documents' tier; bim.bom lists those elements in by_others and keeps only our rough-in")
        .menu(&["Tools", "Building", "Scope by Trade"])]
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let paths = |k: &str| -> Result<Vec<String>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(t)) => Ok(vec![t.clone()]),
            Some(Value::Array(a)) => a
                .iter()
                .take(MAX_FILES)
                .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, format!("`{k}` takes paths (strings)"))))
                .collect(),
            Some(_) => Err(bad(CMD, format!("`{k}` takes a path or a list of paths"))),
        }
    };
    let flag = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(true);
    let (specs, submittals, pdfs) = (paths("specs")?, paths("submittals")?, paths("pdfs")?);
    // A person's decisions first: they are kept whatever the documents say.
    if let Some(a) = p.get("set").and_then(Value::as_array) {
        for x in a.iter().take(64) {
            let class = x["class"].as_str().ok_or_else(|| bad(CMD, "each `set` entry needs its `class`"))?;
            let reason =
                x["reason"].as_str().map(str::trim).filter(|r| !r.is_empty()).ok_or_else(|| bad(CMD, "each `set` entry needs a `reason`"))?;
            answer(s.doc_mut()?, class, x, &format!("set by a person: {reason}")).map_err(|e| bad(CMD, e))?;
        }
    }
    let mut unread = Vec::new();
    let mut docs = file_docs(&specs, &submittals, &mut unread);
    match p.get("text") {
        Some(Value::String(t)) => docs.push(text_doc("the notes given", SourceTier::DrawingRevision, None, t)),
        Some(Value::Array(a)) => {
            for x in a.iter().take(MAX_FILES) {
                let name = x["document"].as_str().unwrap_or("the notes given");
                let tier = x["tier"].as_str().and_then(SourceTier::parse).unwrap_or(SourceTier::DrawingRevision);
                let trade = x["trade"].as_str().and_then(|t| rules().party_named(t)).or_else(|| rules().discipline_party(name));
                docs.push(text_doc(name, tier, trade, x["text"].as_str().unwrap_or_default()));
            }
        }
        _ => {}
    }
    if !pdfs.is_empty() {
        match super::file::io().and_then(|h| h.read_pdf) {
            Some(read_pdf) => {
                for f in pdfs.iter().take(MAX_FILES) {
                    let bytes = match crate::readfile::read(f) {
                        Ok(b) => b,
                        Err(e) => {
                            unread.push(json!({"file": f, "error": e.to_string()}));
                            continue;
                        }
                    };
                    // Page by page: the first says how many.
                    let mut page = 1u64;
                    let mut pages = 1u64;
                    while page <= pages.min(MAX_PDF_PAGES) {
                        match read_pdf(&bytes, &json!({"page": page, "text": true, "images": false})) {
                            Ok((d, report)) => {
                                pages = report["pages"].as_u64().unwrap_or(1);
                                let texts = cadaddy_mepfix::sheet_facts::texts(&d);
                                let number =
                                    cadaddy_mepfix::sheet_facts::identify_sheet(&texts).map_or_else(|| format!("{f} p.{page}"), |id| id.number);
                                docs.push(sheet_doc(&number, &texts));
                            }
                            Err(e) => {
                                unread.push(json!({"file": f, "page": page, "error": e}));
                                break;
                            }
                        }
                        page += 1;
                    }
                }
            }
            None => unread.push(json!({"file": pdfs.join(", "), "error": "no PDF reader in this build"})),
        }
    }
    let sp = super::bspec::current(s, CMD).ok().map(|(sp, _)| sp);
    if flag("sheets")
        && let Some(sp) = &sp
    {
        docs.extend(model_docs(s.doc()?, sp));
    }
    let (stmts, read) = read_docs(&docs);
    record(s.doc_mut()?, &read, "bim.scope");
    let kept = decisions(s.doc()?);
    let explain = p.get("explain").and_then(Value::as_bool).unwrap_or(false);
    let mut out = json!({
        "decisions": kept.iter().map(Decision::to_json).collect::<Vec<_>>(),
        "statements": stmts.len(), "documents": docs.len(), "unread": unread,
    });
    if explain {
        out["read"] = json!(
            stmts
                .iter()
                .take(MAX_EXPLAIN)
                .map(|x| json!({"class": x.class, "says": format!("{:?}", x.says), "source": x.cite.json()}))
                .collect::<Vec<_>>()
        );
        // The paragraphs of the documents asked about, as read.
        let want: Vec<&str> = p.get("explain_documents").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(8).collect();
        out["paragraphs"] = json!(
            docs.iter()
                .filter(|d| want.iter().any(|w| d.name.eq_ignore_ascii_case(w)))
                .flat_map(|d| d
                    .paras
                    .iter()
                    .take(MAX_EXPLAIN)
                    .map(move |x| json!({"document": d.name, "at": x.at, "heading": x.heading, "text": x.text})))
                .take(MAX_EXPLAIN)
                .collect::<Vec<_>>()
        );
    }
    if let Some(mut sp) = sp {
        if flag("kinds") {
            let (n, open) = fa_kinds(s.doc()?, &mut sp);
            out["fire_alarm"] = json!({"resolved": n, "unresolved": open});
        }
        if flag("panels") {
            let rooms = room_boxes(s.doc()?);
            let (placed, not) = place_panels(&mut sp, &panel_places(&docs), &rooms);
            s.doc_mut()?.building_spec = Some(sp.to_json());
            let built = build_panels(s.doc_mut()?, &sp, &placed, &rooms);
            out["panels"] =
                json!({"placed": placed, "built": built, "not_placed": not.iter().map(|(t, w)| json!({"tag": t, "why": w})).collect::<Vec<_>>()});
        } else {
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }
        out["elements"] = summary(&kept, &sp);
        out["questions"] = json!(
            questions(s.doc()?, &sp)
                .into_iter()
                .map(|(c, a, n, dflt)| json!({"class": c, "aspect": a, "elements": n, "default": dflt}))
                .collect::<Vec<_>>()
        );
    }
    s.touch();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(x: f64, y: f64, v: &str) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 }
    }

    fn decision<'a>(ds: &'a [Decision], class: &str) -> &'a Decision {
        ds.iter().find(|d| d.class == class).unwrap_or_else(|| panic!("no {class}: {ds:?}"))
    }

    fn items(d: &Decision) -> Vec<&str> {
        d.ours.iter().map(|i| i.item.as_str()).collect()
    }

    #[test]
    fn the_rules_data_reads() {
        let r = rules();
        assert_eq!(r.ours, "electrical");
        assert!(r.classes.len() >= 3 && r.parties.len() >= 6 && !r.fa_kinds.is_empty() && !r.panels.is_empty(), "{r:?}");
        for c in &r.classes {
            assert!(r.parties.contains_key(&c.party), "{} has party {}", c.class, c.party);
            assert!(c.default_ours.iter().all(|i| r.items.contains_key(i)), "{c:?}");
        }
    }

    #[test]
    fn a_numbered_notes_list_names_the_fire_alarm_contractor_for_the_system() {
        // A made-up electrical sheet: its fire alarm notes as a list under their heading.
        let texts = vec![
            t(100.0, 100.0, "FIRE ALARM NOTES"),
            t(100.0, 98.0, "1. THE DEVICES AND CABLING OF THE WHOLE SYSTEM ARE THE"),
            t(101.0, 96.6, "RESPONSIBILITY OF THE FIRE ALARM CONTRACTOR."),
            t(100.0, 95.0, "2. THE ELECTRICIAN SHALL PROVIDE A 120V BRANCH CIRCUIT FOR"),
            t(101.0, 93.6, "THE FIRE ALARM CONTROL PANEL."),
            t(100.0, 92.0, "3. THE SYSTEM IS A DEFERRED SUBMITTAL."),
        ];
        let doc = sheet_doc("E-301", &texts);
        let (_, ds) = read_docs(&[doc]);
        let fa = decision(&ds, "fire alarm");
        assert_eq!((fa.furnish.as_str(), fa.install.as_str(), fa.party_from.as_str()), ("fire alarm", "fire alarm", "documents"), "{fa:?}");
        assert!(fa.party_cites[0].text.contains("RESPONSIBILITY OF THE FIRE ALARM CONTRACTOR"), "{fa:?}");
        assert_eq!(fa.party_cites[0].doc, "E-301");
        assert_eq!(items(fa), ["power"], "the panel's circuit is ours: {fa:?}");
        assert!(fa.design_build.is_some(), "{fa:?}");
        // The circuit is for the panel, not each detector.
        assert!(fa.ours_for("fire alarm device", "smoke detector").is_empty());
        assert_eq!(fa.ours_for("fire alarm panel", "control panel").len(), 1);
    }

    #[test]
    fn rough_in_only_notes_give_the_low_voltage_devices_away_and_keep_our_box_and_conduit() {
        // A legend's loose note over three lines (the second starts mid-word), and a vendor
        // named elsewhere on another sheet.
        let legend = vec![
            t(10.0, 50.0, "NOTE: ALL LOW VOLTAGE DEVICES ARE ROUGH-IN ONLY. PROVIDE EACH ROUGH-"),
            t(10.0, 48.6, "IN WITH A 4\" SQUARE BOX AND 1\" CONDUIT STUBBED INTO THE"),
            t(10.0, 47.2, "ACCESSIBLE CEILING SPACE ABOVE."),
            t(60.0, 30.0, "DATA OUTLET"),
        ];
        let plan = vec![t(0.0, 0.0, "COORDINATE TV OUTLET LOCATIONS WITH THE LOW VOLTAGE VENDOR BEFORE ROUGH-IN.")];
        let (_, ds) = read_docs(&[sheet_doc("E-302", &legend), sheet_doc("E-303", &plan)]);
        let lv = decision(&ds, "low voltage");
        assert_eq!((lv.furnish.as_str(), lv.party_from.as_str()), ("low voltage", "documents"), "{lv:?}");
        assert!(lv.by_others());
        assert_eq!(items(lv), ["box", "conduit"], "{lv:?}");
        assert_eq!(lv.ours.iter().find(|i| i.item == "conduit").map(|i| i.size.as_str()), Some("1\""));
        assert!(lv.ours_cites[0].text.contains("ROUGH-IN WITH A 4\" SQUARE BOX"), "the run-on lines joined: {lv:?}");
    }

    #[test]
    fn a_mechanical_sheet_order_furnishes_the_thermostat_and_no_box_is_required() {
        // A unit schedule's note on a mechanical sheet (an order: the sheet's trade's), and a
        // specification section of Division 23 putting it in an electrical box.
        let sheet =
            sheet_doc("M-701", &[t(0.0, 0.0, "FURNISH EACH UNIT WITH ITS WALL THERMOSTAT. NO CONDUIT OR BACK BOX IS REQUIRED FOR THE THERMOSTAT.")]);
        assert_eq!(sheet.trade.as_deref(), Some("mechanical"));
        let spec = spec_doc(
            "23 99 00 - Units.pdf",
            "SECTION 239900 - UNITS\n\nPART 3 - EXECUTION\n\nA.\nInstall the thermostats in electrical outlet boxes.\n",
        );
        assert_eq!(spec.trade.as_deref(), Some("mechanical"), "{spec:?}");
        let (_, ds) = read_docs(std::slice::from_ref(&spec));
        let th = decision(&ds, "thermostat");
        assert_eq!((th.install.as_str(), th.party_from.as_str()), ("mechanical", "documents"), "{th:?}");
        assert_eq!(items(th), ["box"], "an electrical outlet box: ours whoever installs into it: {th:?}");
        // With the drawing's note: no box, no conduit (a drawing outranks the specification).
        let (_, ds) = read_docs(&[spec, sheet]);
        let th = decision(&ds, "thermostat");
        assert_eq!((th.furnish.as_str(), th.install.as_str()), ("mechanical", "mechanical"), "{th:?}");
        assert!(th.ours.is_empty(), "{th:?}");
        assert_eq!(th.ours_from, "documents");
        assert!(th.ours_cites.iter().any(|c| c.text.contains("NO CONDUIT OR BACK BOX")), "{th:?}");
    }

    #[test]
    fn a_responsibility_matrix_reads_by_its_columns_in_either_order() {
        // Specification text: table rows 3 spaces apart.
        let spec = "SECTION 230900 - CONTROLS\n\nITEM   FURNISHED BY   INSTALLED BY   WIRED BY\nTHERMOSTATS   MC   EC   EC\nSMOKE DETECTORS   FAC   FAC   FAC\n";
        let (_, ds) = read_docs(&[spec_doc("23 09 00 - Controls.pdf", spec)]);
        let th = decision(&ds, "thermostat");
        assert_eq!((th.furnish.as_str(), th.install.as_str()), ("mechanical", "electrical"), "{th:?}");
        assert!(th.by_others(), "furnished by another trade");
        assert_eq!(items(th), ["box", "conduit"], "wired by us: {th:?}");
        let fa = decision(&ds, "fire alarm");
        assert_eq!((fa.furnish.as_str(), fa.install.as_str()), ("fire alarm", "fire alarm"));
        // The same matrix drawn on a sheet, its columns the other way round.
        let texts = vec![
            t(0.0, 20.0, "EQUIPMENT"),
            t(20.0, 20.0, "INSTALLED BY"),
            t(40.0, 20.0, "FURNISHED BY"),
            t(0.0, 18.0, "TEMPERATURE SENSORS"),
            t(20.0, 18.0, "TCC"),
            t(40.0, 18.0, "OWNER"),
        ];
        let (_, ds) = read_docs(&[sheet_doc("M-702", &texts)]);
        let th = decision(&ds, "thermostat");
        assert_eq!((th.furnish.as_str(), th.install.as_str()), ("owner", "mechanical"), "{th:?}");
    }

    #[test]
    fn furnished_by_constructions_and_by_others() {
        let text = "CARD READERS SHALL BE FURNISHED BY THE OWNER AND INSTALLED BY THE SECURITY CONTRACTOR. ELECTRICAL CONTRACTOR SHALL FURNISH AND INSTALL BACK BOXES AND 3/4\" CONDUIT WITH PULL STRING FOR CARD READERS.";
        let (_, ds) = read_docs(&[text_doc("E-304", SourceTier::DrawingRevision, Some("electrical".into()), text)]);
        let lv = decision(&ds, "low voltage");
        assert_eq!((lv.furnish.as_str(), lv.install.as_str()), ("owner", "low voltage"), "{lv:?}");
        assert_eq!(items(lv), ["box", "conduit", "pull string"], "{lv:?}");
        assert_eq!(lv.ours.iter().find(|i| i.item == "conduit").map(|i| i.size.as_str()), Some("3/4\""));
        // `BY OTHERS`: the party the documents name beside it, else the class's.
        let (_, ds) = read_docs(&[text_doc("E-305", SourceTier::DrawingRevision, Some("electrical".into()), "THERMOSTATS BY OTHERS.")]);
        let th = decision(&ds, "thermostat");
        assert_eq!((th.furnish.as_str(), th.party_from.as_str(), th.ours_from.as_str()), ("mechanical", "documents", "default"), "{th:?}");
    }

    #[test]
    fn our_own_orders_and_refusals_never_give_the_element_to_us() {
        // An electrical sheet's orders name the devices only as where our work goes; `SHALL NOT
        // LOCATE` is no responsibility.
        let text =
            "PROVIDE A STROBE OVER THE SIAMESE CONNECTION. THE ELECTRICAL CONTRACTOR SHALL NOT LOCATE ANY SMOKE DETECTOR WITHIN 3 FEET OF A GRILLE.";
        let (st, ds) = read_docs(&[text_doc("E-306", SourceTier::DrawingRevision, Some("electrical".into()), text)]);
        assert!(!st.iter().any(|s| matches!(&s.says, Says::Element { .. })), "{st:?}");
        assert_eq!(decision(&ds, "fire alarm").party_from, "default");
        // Nothing said: each class its default, for a question.
        let (_, ds) = read_docs(&[]);
        assert!(ds.iter().all(|d| d.party_from == "default" && d.ours_from == "default"), "{ds:?}");
    }

    #[test]
    fn fire_alarm_kinds_by_legend_words() {
        let r = rules();
        let k = |w: &str| r.fa_kind_of(w);
        assert_eq!(k("GUEST ROOM SMOKE DETECTOR WITH SOUNDER BASE").as_deref(), Some("smoke detector"));
        assert_eq!(k("SMOKE/CARBON MONOXIDE DETECTOR").as_deref(), Some("smoke/CO detector"));
        assert_eq!(k("DUCT SMOKE DETECTOR").as_deref(), Some("duct detector"));
        assert_eq!(k("HEAT DETECTOR, RATE OF RISE").as_deref(), Some("heat detector"));
        assert_eq!(k("DUAL ACTION PULL STATION").as_deref(), Some("pull station"));
        assert_eq!(k("F.A. HORN/STROBE, WALL").as_deref(), Some("horn/strobe"));
        assert_eq!(k("WALL STROBE").as_deref(), Some("strobe"));
        assert_eq!(k("DOOR HOLDER, MAGNETIC").as_deref(), Some("door holder"));
        assert_eq!(k("CONTROL RELAY MODULE").as_deref(), Some("relay/module"));
        assert_eq!(k("REMOTE ANNUNCIATOR").as_deref(), Some("annunciator"));
        assert_eq!(k("RECEPTACLE"), None);
    }

    #[test]
    fn a_panels_room_comes_from_the_notes() {
        let text = "THE FIRE ALARM CONTROL PANEL WILL BE LOCATED ON THE 6TH FLOOR IN ELECTRICAL ROOM 610 WITH A REMOTE ANNUNCIATOR IN LOBBY 605AND ITS DEVICES THROUGHOUT.";
        let ps = panel_places(&[text_doc("FA-501", SourceTier::Submittal, Some("fire alarm".into()), text)]);
        assert_eq!(ps.len(), 2, "{ps:?}");
        assert_eq!((ps[0].tag.as_str(), ps[0].room.as_str(), ps[0].floor), ("FACP", "610", Some(6)));
        assert_eq!(ps[0].words, "ELECTRICAL ROOM");
        assert_eq!((ps[1].kind.as_str(), ps[1].room.as_str()), ("annunciator", "605"));
        // Placed along a wall of its room.
        let room = |n: &str, level: &str, x: f64| super::super::gearbuild::RoomBox {
            level: level.into(),
            number: n.into(),
            name: String::new(),
            outline: vec![Vec2::new(x, 0.0), Vec2::new(x + 120.0, 0.0), Vec2::new(x + 120.0, 96.0), Vec2::new(x, 96.0)],
            offset: Vec2::ZERO,
        };
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        let (placed, not) = place_panels(&mut sp, &ps, &[room("610", "Level 6", 0.0), room("510", "Level 5", 500.0)]);
        assert_eq!(placed, ["FACP"], "{not:?}");
        assert_eq!(not.len(), 1, "no room 605: {not:?}");
        let e = &sp.mep.equipment[0];
        assert_eq!((e.kind.as_str(), e.level.as_str()), ("fire alarm panel", "Level 6"));
        assert!(e.min[0] >= 0.0 && e.max[0] <= 120.0 && e.min[1] >= 0.0 && e.max[1] <= 96.0, "{e:?}");
        assert!((e.bottom_ft + e.height_ft - 6.0).abs() < 1e-9, "its top at 72\": {e:?}");
        assert!(e.sources["position"].contains("ELECTRICAL ROOM 610"));
    }

    #[test]
    fn hostile_text_never_panics() {
        let long = format!("{}{}REQUIRED", "BY ".repeat(5_000), "NO ".repeat(5_000));
        for text in
            [long.as_str(), "", ".", "BY", "SHALL", "RESPONSIBILITY OF", "FURNISHED BY", "NO", "PROVIDE", "IN 1", "FACP IN", "\u{201c}\u{2019}"]
        {
            let (_, _) = read_docs(&[text_doc("x", SourceTier::Spec, Some("mechanical".into()), &format!("THERMOSTAT {text}"))]);
            let _ = panel_places(&[text_doc("x", SourceTier::Spec, None, &format!("FACP {text}"))]);
        }
        let texts: Vec<TextItem> = (0..2_000).map(|i| t(0.0, -f64::from(i) * 1.4, "FIRE ALARM CONTRACTOR SHALL PROVIDE")).collect();
        let d = sheet_doc("E-307", &texts);
        assert!(!d.paras.is_empty());
    }

    #[test]
    fn a_line_split_into_runs_is_read_whole() {
        // An import's runs of one line: the quoted model number in a run of its own.
        let runs = vec![
            TextItem { value: "PROVIDE WITH WIRED WALL \"".into(), at: Vec2::new(10.0, 20.0), height: 0.1, rotation: 0.0 },
            TextItem { value: "XY-1\" THERMOSTAT.".into(), at: Vec2::new(11.4, 20.0), height: 0.1, rotation: 0.0 },
            TextItem { value: "NO CONDUIT OR JUNCTION BOX SHALL BE REQUIRED FOR".into(), at: Vec2::new(12.5, 20.0), height: 0.1, rotation: 0.0 },
            TextItem { value: "THERMOSTAT INSTALLATION.".into(), at: Vec2::new(15.4, 20.0), height: 0.1, rotation: 0.0 },
            // Another column far to the right stays apart.
            TextItem { value: "REFER TO THE SCHEDULE".into(), at: Vec2::new(40.0, 20.0), height: 0.1, rotation: 0.0 },
        ];
        let joined = join_runs(&runs);
        assert_eq!(joined.len(), 2, "{joined:?}");
        let (_, ds) = read_docs(&[sheet_doc("M-703", &runs)]);
        let th = decision(&ds, "thermostat");
        assert_eq!((th.ours_from.as_str(), th.ours.len()), ("documents", 0), "{th:?}");
    }

    #[test]
    fn an_elements_own_name_a_coordination_and_a_tables_run_of_cells_say_nothing() {
        let r = rules();
        // A manual fire-alarm box is the pull station, not a back box.
        assert!(items_in(r, &toks("MANUAL FIRE-ALARM BOX ON A CONTRASTING BACKGROUND")).is_empty());
        assert_eq!(items_in(r, &toks("A 4\" SQUARE BACK BOX")).first().map(|i| (i.0.as_str(), i.1.as_str())), Some(("box", "4\"")));
        // `CONFIRM … WITH OWNER`: who to ask, not who furnishes.
        let text = "ALL LOW VOLTAGE DEVICES ARE ROUGH-IN ONLY. COORDINATE THE LOW VOLTAGE LAYOUT WITH THE OWNER.";
        let (_, ds) = read_docs(&[text_doc("E-308", SourceTier::DrawingRevision, Some("electrical".into()), text)]);
        assert_eq!(decision(&ds, "low voltage").furnish, "low voltage");
        // A shop drawing's legend run together: no statement from its words.
        let cells = format!("DEVICE LEGEND {} PROVIDED BY OTHERS STROBE BACK BOX", "4\" SQUARE DEEP BOX CEILING STROBE ".repeat(12));
        let (st, _) = read_docs(&[text_doc("FA-502", SourceTier::Submittal, Some("fire alarm".into()), &cells)]);
        assert!(st.is_empty(), "{st:?}");
        // The circuit ordered, not the panel it goes to.
        let (st, _) = read_docs(&[text_doc(
            "FA-502",
            SourceTier::Submittal,
            Some("fire alarm".into()),
            "THE ELECTRICIAN SHALL FURNISH A 120V CIRCUIT TO THE PANEL OF THE FIRE ALARM SYSTEM IN ITS BACK BOX.",
        )]);
        assert!(
            st.iter().any(|s| matches!(&s.says, Says::Items { items, .. } if items.iter().map(|i| i.0.as_str()).collect::<Vec<_>>() == ["power"])),
            "{st:?}"
        );
    }

    #[test]
    fn words_an_import_ran_together_are_read_apart() {
        assert_eq!(toks("SHALL BE REQUIREDFOR THE"), ["SHALL", "BE", "REQUIRED", "FOR", "THE"]);
        assert_eq!(toks("WITH LOW VOLTAGEVENDOR"), ["WITH", "LOW", "VOLTAGE", "VENDOR"]);
        assert_eq!(toks("LOBBY 605AND ROOM 601B"), ["LOBBY", "605", "AND", "ROOM", "601B"]);
        // A word of its own stays whole.
        assert_eq!(toks("BOXES CONDUITS THERMOSTAT INSTALLATION"), ["BOXES", "CONDUITS", "THERMOSTAT", "INSTALLATION"]);
        let (_, ds) = read_docs(&[text_doc(
            "M-704",
            SourceTier::DrawingRevision,
            Some("mechanical".into()),
            "PROVIDE WITH WIRED WALL THERMOSTAT. NO CONDUIT OR JUNCTION BOX SHALL BE REQUIREDFOR THERMOSTAT INSTALLATION.",
        )]);
        assert!(decision(&ds, "thermostat").ours.is_empty());
    }

    #[test]
    fn a_new_reading_replaces_the_last_whatever_its_tier_and_keeps_an_answer() {
        let mut d = cadaddy_doc::Drawing::new_imperial();
        // First read: a submittal says the thermostat's box is ours.
        let sub = text_doc(
            "Submittal 1",
            SourceTier::Submittal,
            Some("mechanical".into()),
            "ELECTRICAL CONTRACTOR SHALL PROVIDE BACK BOXES FOR THERMOSTATS.",
        );
        record(&mut d, &read_docs(&[sub]).1, "test");
        assert_eq!(items(decision(&decisions(&d), "thermostat")), ["box"]);
        // Read again with a drawing that says none is needed: it holds now.
        let dwg = text_doc("M-705", SourceTier::DrawingRevision, Some("mechanical".into()), "NO JUNCTION BOX IS REQUIRED FOR THE THERMOSTAT.");
        record(&mut d, &read_docs(std::slice::from_ref(&dwg)).1, "test");
        let th = decisions(&d);
        let th = decision(&th, "thermostat");
        assert!(th.ours.is_empty() && th.ours_from == "documents", "{th:?}");
        // A person's answer to the party stays through a reading that says nothing of it.
        answer(&mut d, "thermostat", &json!({"furnish": "owner", "install": "mechanical"}), "answered (user)").unwrap();
        record(&mut d, &read_docs(&[dwg]).1, "test");
        let th = decisions(&d);
        assert_eq!((decision(&th, "thermostat").furnish.as_str(), decision(&th, "thermostat").party_from.as_str()), ("owner", "answer"));
    }

    #[test]
    fn a_general_note_outranks_a_keynote_for_every_element() {
        // The keynote (a guest room's run) read first, the legend's blanket note after.
        let key = sheet_doc(
            "E-309",
            &[t(100.0, 100.0, "ELECTRICAL KEYNOTES"), t(100.0, 98.0, "4. PROVIDE A 1\" LOW VOLTAGE CONDUIT WITH PULL STRING UP TO THE HALL.")],
        );
        let legend =
            sheet_doc("E-310", &[t(0.0, 0.0, "NOTE: ALL LOW VOLTAGE DEVICES ARE ROUGH-IN ONLY. PROVIDE EACH WITH A BOX AND 3/4\" CONDUIT.")]);
        let (_, ds) = read_docs(&[key, legend]);
        let lv = decision(&ds, "low voltage");
        assert_eq!(lv.ours.iter().find(|i| i.item == "conduit").map(|i| i.size.as_str()), Some("3/4\""), "{lv:?}");
        assert_eq!(items(lv), ["box", "conduit", "pull string"], "the keynote's pull string still ours: {lv:?}");
        assert_eq!(lv.ours_cites[0].doc, "E-310");
    }
}
