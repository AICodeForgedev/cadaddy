//! G-COVERAGE (Q6183): a whole KIND of object the prints show and the model doesn't hold.
//!
//! Validate compares the prints with the model kind by kind, independently of the build's own
//! counts.
//!
//! Reading the kinds the documents define ([`Defs`]):
//! - every sheet's legends and device schedules (`device_types::read_legend`: a type tag in the
//!   first column and the words after it), each row's sample symbol (the block drawn on its row)
//!   and the `KEY=MEANING` designations;
//! - every abbreviation list (a heading naming abbreviations and the code–meaning pairs under
//!   it) whose meaning names an object (`validate_coverage.json` `object_nouns`);
//! - every keynote list's notes that provide an object (`PROVIDE … FIRE/SMOKE DAMPER`): the
//!   markers of that note are its marks;
//! - the print inventory's kinds (`bim.inventory`: its legend and schedule rows, `legend:<words>`
//!   kinds, its object marks' kinds).
//!
//! A kind defined on a trade's sheet is that trade's (its codes are looked for on that trade's
//! plans); a general sheet's is every trade's. Two definitions with the same words, or the same
//! code within a trade, are one kind.
//!
//! Counting (per sheet and level): each code printed as a tag on a plan (a text, a block's
//! attribute, `CODE-n` numbered), each sample symbol drawn on a plan, each keynote marker. Every
//! mark goes onto its level's plan by its sheet's registration; the same kind on a floor plan
//! and an enlarged plan within the rule's `dedupe_ft` counts once. A unit type's enlarged plan
//! counts once per room of that type (its views' most), and a floor plan's mark in such a room
//! is the typical plan's.
//!
//! The model's elements (as the prescreen reads them: their own kind, else what their records,
//! the job's tables or the project say) count for the kind they match best: their mark is one of
//! its codes, their kind or type words are its words.
//!
//! Verdicts ([`check`]):
//! - marks on the plans and ZERO model elements: `fail`, ranked first ("<kind>: N marks on
//!   <sheets>, 0 in the model");
//! - a model count off the plans' by more than the rule's `count_abs` / `count_share`: `fail`,
//!   with both counts and the sheets;
//! - a kind the electrical, fire alarm or low-voltage sheets connect to (a note gives it power,
//!   a circuit, a fire alarm module, a data drop) with no model element: `fail`, even when its own
//!   trade is by others;
//! - the inventory's unexplained marks, grouped by what and text, repeated on several plan sheets
//!   (`repeat_sheets`, `repeat_marks`): `unsure`, a likely missing kind, with examples;
//! - else `pass`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::device_types;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::levels::SheetLink;
use serde::Deserialize;
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_packet::{Area, inside, norm_rect, point_in, pt, r2, trade_of_discipline};
use super::validate_rules::Effective;

pub const RULE: &str = "G-COVERAGE";
/// The verdicts G-COVERAGE gives.
pub const VERDICTS: [&str; 3] = ["pass", "fail", "unsure"];
const DATA: &str = include_str!("validate_coverage.json");
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Texts and symbols kept per sheet, at most.
const MAX_SHEET_TEXTS: usize = 200_000;
const MAX_SHEET_SYMBOLS: usize = 200_000;
/// Sheets read, kinds kept, marks counted, at most.
const MAX_SHEETS: usize = 2_000;
/// A PDF read for its legend pages, at most this big.
const MAX_PDF_BYTES: usize = 200 * 1024 * 1024;
const MAX_DEFS: usize = 5_000;
const MAX_MARKS: usize = 2_000_000;
/// Elements kept per kind for a kind with none of its own, at most.
const MAX_SHARED: usize = 100_000;
/// Sources, connections and handles listed per kind, examples per group, at most.
const MAX_SOURCES: usize = 8;
const MAX_LISTED: usize = 12;
const MAX_EXAMPLES: usize = 5;
/// Rows of the coverage table, at most.
const MAX_ROWS: usize = 2_000;
/// A tag's longest text (a code with its number).
const MAX_TAG_CHARS: usize = 14;
/// A note's longest text read, and quoted.
const MAX_NOTE_CHARS: usize = 1_200;
const QUOTE_CHARS: usize = 200;
/// A legend row's words end at a gap this many of its text heights wide (the next column).
const COLUMN_GAP_H: f64 = 8.0;
/// An abbreviation's longest meaning.
const MAX_MEANING: usize = 60;
/// A row's sample symbol sits within this many of its row's text heights of the row's tag.
const SAMPLE_REACH_H: f64 = 12.0;
/// The same tag printed twice on one sheet (a text and an attribute) within this many heights.
const SAME_TAG_H: f64 = 1.0;
/// Defaults of the rule's tolerances.
const DEFAULT_SHARE: f64 = 0.1;
const DEFAULT_ABS: f64 = 1.0;
const DEFAULT_DEDUPE_FT: f64 = 3.0;
const DEFAULT_REPEAT_SHEETS: f64 = 2.0;
const DEFAULT_REPEAT_MARKS: f64 = 4.0;

// ---------------------------------------------------------------- rules data

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Connection {
    via: String,
    words: Vec<String>,
}

/// `validate_coverage.json`: trade vocabulary with its source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Data {
    source: String,
    object_nouns: Vec<String>,
    compound_nouns: Vec<String>,
    stop_words: Vec<String>,
    phrase_breaks: Vec<String>,
    not_new_words: Vec<String>,
    prepositions: Vec<String>,
    provide_words: Vec<String>,
    connections: Vec<Connection>,
    connecting_trades: Vec<String>,
    abbreviation_headings: Vec<String>,
    /// Drafting words: a meaning holding one names a sheet's mark (a plan, a section, a symbol),
    /// no object.
    not_object_words: Vec<String>,
    /// Words of a sheet's title or type that make its page worth reading for definitions.
    page_words: Vec<String>,
    /// The connecting trades' own objects (a disconnect, a box): never another trade's.
    own_nouns: Vec<String>,
    max_pages: usize,
    max_code_chars: usize,
    max_phrase_words: usize,
    max_groups: usize,
}

fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(|| {
        let mut d: Data = serde_json::from_str(DATA).unwrap_or_default();
        d.max_code_chars = d.max_code_chars.clamp(1, 12);
        d.max_phrase_words = d.max_phrase_words.clamp(1, 8);
        d.max_groups = d.max_groups.clamp(1, 500);
        d.max_pages = d.max_pages.min(200);
        d
    })
}

fn listed(list: &[String], w: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(w))
}

// ---------------------------------------------------------------- words

/// A word as compared: upper case, a plural's `S` off (`DAMPERS` → `DAMPER`).
fn singular(w: &str) -> String {
    let u = w.to_ascii_uppercase();
    if u.len() > 3 && u.ends_with('S') && !u.ends_with("SS") {
        if let Some(stem) = u.strip_suffix("IES") {
            return format!("{stem}Y");
        }
        if let Some(stem) = u.strip_suffix("ES").filter(|s| s.ends_with("CH") || s.ends_with("SH") || s.ends_with('X')) {
            return stem.to_string();
        }
        return u.trim_end_matches('S').to_string();
    }
    u
}

/// The words of a text: letters and digits, upper case.
fn words_of(s: &str) -> Vec<String> {
    s.chars()
        .take(MAX_NOTE_CHARS)
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// A kind's key: its words, lower case and singular, without stop words, numbers and lone
/// letters (`FIRE/SMOKE DAMPERS` → `fire smoke damper`, `fire_smoke_damper` → the same).
pub(crate) fn key_of(s: &str) -> String {
    let d = data();
    let s = s.trim();
    let s = s.strip_prefix("legend:").unwrap_or(s);
    let mut out: Vec<String> = Vec::new();
    for w in words_of(s) {
        if w.len() < 2 || w.chars().any(|c| c.is_ascii_digit()) || listed(&d.stop_words, &w) {
            continue;
        }
        let w = singular(&w).to_ascii_lowercase();
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out.join(" ")
}

fn tokens(key: &str) -> BTreeSet<&str> {
    key.split(' ').filter(|w| !w.is_empty()).collect()
}

/// The last object noun of a key, its head (`fire smoke damper` → `damper`).
fn head(key: &str) -> Option<&str> {
    let d = data();
    key.split(' ').rev().find(|w| listed(&d.object_nouns, w))
}

/// Whether a meaning names an object (one of the rules data's nouns, or a compound).
fn names_object(meaning: &str) -> bool {
    let d = data();
    let ws: Vec<String> = words_of(meaning).iter().map(|w| singular(w)).collect();
    let joined = format!(" {} ", ws.join(" "));
    ws.iter().any(|w| listed(&d.object_nouns, w)) || d.compound_nouns.iter().any(|c| joined.contains(&format!(" {} ", c.to_ascii_uppercase())))
}

/// A legend description or meaning as a kind's name: its words before the first comma,
/// parenthesis or dash, the numbers and ratings off (`DUPLEX RECEPTACLE, 20A, 125V` →
/// `DUPLEX RECEPTACLE`).
fn name_of(description: &str) -> String {
    let cut = description.find([',', '(', ';', '[']).map_or(description, |i| description.get(..i).unwrap_or(description));
    let cut = cut.split(" - ").next().unwrap_or(cut);
    let ws: Vec<&str> =
        cut.split_whitespace().filter(|w| w.chars().any(|c| c.is_ascii_alphabetic()) && !w.chars().any(|c| c.is_ascii_digit())).take(6).collect();
    ws.join(" ").trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_uppercase()
}

/// The objects a note names: each object noun with the words before it (up to the rules data's
/// most), as (key, name, whether a preposition comes right before it, whether the note calls
/// for it new: a provide word before it, or it starts the note).
fn phrases(text: &str) -> Vec<(String, String, bool, bool)> {
    let d = data();
    let up = text.chars().take(MAX_NOTE_CHARS).collect::<String>().to_ascii_uppercase();
    // Words, with the punctuation that ends a phrase as its own piece.
    let mut ws: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in up.chars() {
        if c.is_ascii_alphanumeric() || c == '\'' {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                ws.push(std::mem::take(&mut cur));
            }
            if matches!(c, ',' | ';' | '.' | ':' | '(' | ')') {
                ws.push(".".into());
            }
        }
    }
    if !cur.is_empty() {
        ws.push(cur);
    }
    let sing: Vec<String> = ws.iter().map(|w| singular(w)).collect();
    let is_noun = |i: usize| -> bool {
        let Some(w) = sing.get(i) else { return false };
        if listed(&d.object_nouns, w) {
            return true;
        }
        let prev = i.checked_sub(1).and_then(|p| sing.get(p));
        prev.is_some_and(|p| listed(&d.compound_nouns, &format!("{p} {w}")))
    };
    let mut out = Vec::new();
    for i in 0..sing.len() {
        if !is_noun(i) || is_noun(i + 1) {
            continue;
        }
        let mut start = i;
        while start > 0 && i + 1 - start < d.max_phrase_words {
            let Some(p) = sing.get(start - 1) else { break };
            let breaks = p == "."
                || p.chars().any(|c| c.is_ascii_digit())
                || listed(&d.phrase_breaks, p)
                || listed(&d.prepositions, p)
                || (listed(&d.stop_words, p) && !is_noun(start - 1));
            if breaks {
                break;
            }
            start -= 1;
        }
        let Some(span) = sing.get(start..=i) else { continue };
        if span.iter().any(|w| listed(&d.not_new_words, w))
            || start.checked_sub(1).and_then(|p| sing.get(p)).is_some_and(|p| listed(&d.not_new_words, p))
        {
            continue;
        }
        let name = ws.get(start..=i).map(|s| s.join(" ")).unwrap_or_default();
        let key = key_of(&name);
        if key.is_empty() {
            continue;
        }
        let after_prep = start.checked_sub(1).and_then(|p| sing.get(p)).is_some_and(|p| listed(&d.prepositions, p));
        // Called for new: a provide word earlier in its sentence, or it starts the note.
        let sentence_start = sing.get(..start).and_then(|b| b.iter().rposition(|w| w == ".")).map_or(0, |p| p + 1);
        let provided = start == 0 || sing.get(sentence_start..start).is_some_and(|b| b.iter().any(|w| listed(&d.provide_words, w)));
        out.push((key, name, after_prep, provided));
    }
    out
}

/// What a note connects (power, a fire alarm module, a data drop), by its words.
fn connection_via(text: &str) -> Option<&'static str> {
    let d = data();
    let joined = format!(" {} ", words_of(text).join(" "));
    for c in &d.connections {
        if c.words.iter().any(|w| joined.contains(&format!(" {} ", w.to_ascii_uppercase()))) {
            return match c.via.as_str() {
                "power" => Some("power"),
                "fire_alarm" => Some("fire_alarm"),
                "data" => Some("data"),
                _ => Some("connection"),
            };
        }
    }
    None
}

fn quote(t: &str) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= QUOTE_CHARS { t } else { format!("{}…", t.chars().take(QUOTE_CHARS).collect::<String>()) }
}

/// How near two prints of one tag lie (a text and its block's attribute): the larger of their
/// heights, at most four times the smaller (an odd giant text swallows no tags round it); an
/// unknown height (0) takes the other's.
fn same_reach(a: f64, b: f64) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    if lo <= 0.0 { hi } else { hi.min(4.0 * lo) }
}

/// The code a tag prints: the whole text, or its letters before a number (`FSD-1`, `EF 2`,
/// `FSD18`); `None` for lettering longer than a tag.
fn tag_codes(t: &str) -> Vec<String> {
    let t = t.trim().trim_matches(|c| c == '(' || c == ')').trim().to_ascii_uppercase();
    if t.is_empty() || t.chars().count() > MAX_TAG_CHARS || t.contains("  ") {
        return Vec::new();
    }
    let mut out = vec![t.clone()];
    let letters: String = t.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let rest = t.get(letters.len()..).unwrap_or("").trim_start_matches(['-', ' ', '.']);
    if letters.len() >= 2 && letters.len() < t.len() && !rest.is_empty() && rest.starts_with(|c: char| c.is_ascii_digit()) {
        let tail_ok = rest.chars().all(|c| c.is_ascii_alphanumeric());
        if tail_ok {
            out.push(letters);
        }
    }
    out
}

// ---------------------------------------------------------------- the sheets

/// One sheet of the set the drawing holds.
struct Sheet {
    number: String,
    rect: [f64; 4],
    /// Its trade (`electrical`, `mechanical`… `general`).
    trade: &'static str,
    /// Whether some link puts it (or a view of it) on a level: a plan.
    plan: bool,
    texts: Vec<Txt>,
    symbols: Vec<(String, [f64; 2])>,
    /// Notes: an MText's lines joined, and each long line of lettering.
    notes: Vec<(String, [f64; 2])>,
    /// Tables read on it (legends, abbreviation lists, keynote lists): their lettering is no tag.
    tables: Vec<[f64; 4]>,
}

#[derive(Clone, Debug)]
struct Txt {
    text: String,
    at: [f64; 2],
    h: f64,
}

fn discipline_of(l: &SheetLink) -> String {
    l.discipline.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| l.number.chars().take_while(|c| c.is_ascii_alphabetic()).collect())
}

/// A sheet's trade by its discipline: a building trade, else `civil`, `landscape`, `structural`
/// or `other` (their kinds are marked on their own sheets), `general` only for a general sheet.
fn trade_of(discipline: &str) -> &'static str {
    let t = trade_of_discipline(discipline);
    if t != "general" {
        return t;
    }
    match discipline.trim().to_ascii_uppercase().chars().next() {
        None | Some('G') => "general",
        Some('C') => "civil",
        Some('L') => "landscape",
        Some('S') => "structural",
        _ => "other",
    }
}

fn new_sheet(number: &str, rect: [f64; 4], trade: &'static str, plan: bool) -> Sheet {
    Sheet { number: number.to_string(), rect, trade, plan, texts: Vec::new(), symbols: Vec::new(), notes: Vec::new(), tables: Vec::new() }
}

fn read_sheets(d: &Drawing, sp: &BuildingSpec) -> Vec<Sheet> {
    let mut sheets: Vec<Sheet> = Vec::new();
    for l in &sp.sheets {
        let Some(r) = l.rect.map(norm_rect).filter(|r| r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]) else { continue };
        let plan = l.level.as_deref().is_some_and(|x| !x.trim().is_empty()) || !l.levels.is_empty();
        if let Some(s) = sheets.iter_mut().find(|s| s.number.eq_ignore_ascii_case(&l.number) && s.rect == r) {
            s.plan |= plan;
            continue;
        }
        if sheets.len() >= MAX_SHEETS {
            break;
        }
        sheets.push(new_sheet(&l.number, r, trade_of(&discipline_of(l)), plan));
    }
    fill(d, &mut sheets);
    sheets
}

/// Each sheet's lettering, symbols and notes: what lies in its rectangle.
fn fill(d: &Drawing, sheets: &mut [Sheet]) {
    if sheets.is_empty() {
        return;
    }
    let which = |p: [f64; 2]| sheets.iter().position(|s| inside(s.rect, p));
    let mut texts: Vec<Vec<Txt>> = sheets.iter().map(|_| Vec::new()).collect();
    let mut syms: Vec<Vec<(String, [f64; 2])>> = sheets.iter().map(|_| Vec::new()).collect();
    let mut notes: Vec<Vec<(String, [f64; 2])>> = sheets.iter().map(|_| Vec::new()).collect();
    let ok = |p: Vec2, h: f64| p.x.is_finite() && p.y.is_finite() && h.is_finite() && h > 0.0;
    for e in d.model.iter().take(MAX_ENTITIES) {
        match &e.kind {
            EntityKind::Text(t) => {
                let (p, h) = (t.insert.xy(), t.height.abs());
                let v = t.value.trim();
                if v.is_empty() || !ok(p, h) {
                    continue;
                }
                let Some(i) = which([p.x, p.y]) else { continue };
                if let Some(list) = texts.get_mut(i).filter(|l| l.len() < MAX_SHEET_TEXTS) {
                    list.push(Txt { text: v.chars().take(MAX_NOTE_CHARS).collect(), at: [p.x, p.y], h });
                }
                if v.len() > 12
                    && let Some(n) = notes.get_mut(i).filter(|l| l.len() < MAX_SHEET_TEXTS)
                {
                    n.push((v.chars().take(MAX_NOTE_CHARS).collect(), [p.x, p.y]));
                }
            }
            EntityKind::MText(t) => {
                let (p, h) = (t.insert.xy(), t.height.abs());
                if !ok(p, h) {
                    continue;
                }
                let Some(i) = which([p.x, p.y]) else { continue };
                let plain = cadaddy_fonts::plain_mtext(&t.contents);
                if let Some(list) = texts.get_mut(i) {
                    for (k, line) in plain.lines().take(200).enumerate() {
                        let v = line.trim();
                        if !v.is_empty() && list.len() < MAX_SHEET_TEXTS {
                            list.push(Txt { text: v.chars().take(MAX_NOTE_CHARS).collect(), at: [p.x, p.y - k as f64 * h * 1.6], h });
                        }
                    }
                }
                let joined = plain.split_whitespace().collect::<Vec<_>>().join(" ");
                if joined.len() > 12
                    && let Some(n) = notes.get_mut(i).filter(|l| l.len() < MAX_SHEET_TEXTS)
                {
                    n.push((joined.chars().take(MAX_NOTE_CHARS).collect(), [p.x, p.y]));
                }
            }
            EntityKind::Insert(ins) => {
                let p = ins.insert.xy();
                if !(p.x.is_finite() && p.y.is_finite()) {
                    continue;
                }
                let Some(i) = which([p.x, p.y]) else { continue };
                if let Some(list) = syms.get_mut(i).filter(|l| l.len() < MAX_SHEET_SYMBOLS) {
                    list.push((ins.block.to_string(), [p.x, p.y]));
                }
                // A tag drawn as a block: its shown attributes, and its tag attribute.
                for a in ins.attribs.iter().filter(|a| !a.invisible || a.tag.eq_ignore_ascii_case("TAG")).take(20) {
                    let v = a.text.value.trim();
                    let (ap, h) = (a.text.insert.xy(), a.text.height.abs());
                    let (ap, h) = if ok(ap, h) && inside(sheets.get(i).map_or([0.0; 4], |s| s.rect), [ap.x, ap.y]) {
                        (ap, h)
                    } else {
                        (p, if h.is_finite() && h > 0.0 { h } else { 0.0 })
                    };
                    if !v.is_empty()
                        && let Some(list) = texts.get_mut(i).filter(|l| l.len() < MAX_SHEET_TEXTS)
                    {
                        list.push(Txt { text: v.chars().take(MAX_NOTE_CHARS).collect(), at: [ap.x, ap.y], h });
                    }
                }
            }
            _ => {}
        }
    }
    for (i, s) in sheets.iter_mut().enumerate() {
        s.texts = texts.get_mut(i).map(std::mem::take).unwrap_or_default();
        s.symbols = syms.get_mut(i).map(std::mem::take).unwrap_or_default();
        s.notes = notes.get_mut(i).map(std::mem::take).unwrap_or_default();
    }
}

/// The job, as the drawing keeps it (`job.start`'s state), when there is one.
fn job_of(d: &Drawing) -> Option<super::guided::GuidedJob> {
    let agent: Value = serde_json::from_str(d.agent.as_deref()?).ok()?;
    serde_json::from_value(agent.get("guided_job")?.clone()).ok()
}

/// The set's sheets the drawing doesn't hold whose page may define kinds (a legend, a schedule,
/// an abbreviation list, general notes: its title or type says so), read off the job's PDFs
/// (the job's sheet index gives each one's file and page). What can't be read is said, by why.
fn read_pages(d: &Drawing, loaded: &BTreeSet<String>, unmeasured: &mut BTreeMap<String, u64>) -> Vec<Sheet> {
    let mut out = Vec::new();
    let Some(job) = job_of(d) else { return out };
    let dt = data();
    let entries: Vec<&Value> =
        job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).into_iter().flatten().take(5_000).collect();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let mut wanted: Vec<(String, String, String, u64)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for e in entries {
        if !text(e, "superseded_by").is_empty() {
            continue;
        }
        let number = text(e, "number");
        let base = number.split('/').next().unwrap_or("").trim().to_ascii_uppercase();
        if base.is_empty() || loaded.contains(&base) || !cadaddy_mepfix::sheet_facts::is_sheet_number(&base) || !seen.insert(base.clone()) {
            continue;
        }
        let what = format!("{} {} {}", text(e, "title"), text(e, "type"), text(e, "sheet_type")).to_ascii_uppercase();
        if !dt.page_words.iter().any(|w| what.contains(&w.to_ascii_uppercase())) {
            continue;
        }
        let (file, page) = (text(e, "file"), e.get("page").and_then(Value::as_u64).filter(|p| *p >= 1).unwrap_or(1));
        if file.is_empty() {
            *unmeasured.entry("a legend or schedule sheet the job keeps no PDF file for".into()).or_default() += 1;
            continue;
        }
        let disc =
            Some(text(e, "discipline")).filter(|x| !x.is_empty()).unwrap_or_else(|| base.chars().take_while(|c| c.is_ascii_alphabetic()).collect());
        wanted.push((base, disc, file, page));
    }
    if wanted.is_empty() {
        return out;
    }
    let Some(read_pdf) = super::file::io().and_then(|h| h.read_pdf) else {
        unmeasured.insert("legend and schedule sheets the drawing doesn't hold: no PDF reader in this build".into(), wanted.len() as u64);
        return out;
    };
    let dir = std::path::PathBuf::from(&job.pdfs_path);
    let dir = if dir.is_file() { dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default() } else { dir };
    let mut files: BTreeMap<String, std::result::Result<Vec<u8>, String>> = BTreeMap::new();
    let total = wanted.len();
    for (number, disc, file, page) in wanted.into_iter().take(dt.max_pages) {
        let named = std::path::PathBuf::from(&file);
        let path = if named.is_absolute() && named.is_file() { named } else { dir.join(named.file_name().unwrap_or_default()) };
        let key = path.to_string_lossy().to_string();
        let bytes = files.entry(key).or_insert_with(|| {
            crate::readfile::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| if b.len() > MAX_PDF_BYTES { Err("the file is over 200 MB".to_string()) } else { Ok(b) })
        });
        let got = match bytes {
            Ok(b) => read_pdf(b, &json!({"page": page, "text": true, "images": false})).map(|(p, _)| p),
            Err(e) => Err(e.clone()),
        };
        match got {
            Ok(pd) => {
                let b = pd.extents(&cadaddy_doc::Space::Model);
                let rect = [b.min.x, b.min.y, b.max.x, b.max.y];
                if b.is_empty() || !rect.iter().all(|v| v.is_finite()) {
                    continue;
                }
                let mut one = vec![new_sheet(&number, rect, trade_of(&disc), false)];
                fill(&pd, &mut one);
                out.extend(one);
            }
            Err(why) => {
                *unmeasured.entry(format!("a legend or schedule sheet's page could not be read ({})", quote(&why))).or_default() += 1;
            }
        }
    }
    if total > dt.max_pages {
        unmeasured.insert("legend and schedule sheets past the rules data's max_pages, not read".into(), (total - dt.max_pages) as u64);
    }
    out
}

fn items_of(texts: &[Txt]) -> Vec<TextItem> {
    texts.iter().map(|t| TextItem { value: t.text.clone(), at: Vec2::new(t.at[0], t.at[1]), height: t.h, rotation: 0.0 }).collect()
}

// ---------------------------------------------------------------- the kinds

/// Where a kind's definition came from.
#[derive(Clone, Debug)]
struct Source {
    from: &'static str,
    sheet: String,
    text: String,
}

/// A kind of object the documents define.
#[derive(Clone, Debug, Default)]
struct Def {
    key: String,
    name: String,
    /// The tags that mark it on plans (upper case).
    codes: BTreeSet<String>,
    /// The blocks its legend rows draw as samples.
    symbols: BTreeSet<String>,
    /// The trades whose plans show it (empty: every trade's).
    trades: BTreeSet<&'static str>,
    sources: Vec<Source>,
    /// The keynotes that call for it: (sheet, number).
    keynotes: BTreeSet<(String, String)>,
    /// The electrical, fire alarm and low-voltage notes connecting to it: (sheet, via, text).
    connected: Vec<(String, &'static str, String)>,
}

#[derive(Default)]
struct Defs {
    list: Vec<Def>,
    by_key: HashMap<String, usize>,
}

impl Defs {
    /// Adds a definition, merged into a kind of the same words, or of the same code within a
    /// trade. Returns its index.
    fn add(&mut self, key: String, name: String, code: Option<String>, trade: &'static str, src: Source) -> Option<usize> {
        if key.is_empty() {
            return None;
        }
        let trades: BTreeSet<&'static str> = if trade == "general" { BTreeSet::new() } else { [trade].into_iter().collect() };
        let overlaps = |d: &Def| d.trades.is_empty() || trades.is_empty() || d.trades.iter().any(|t| trades.contains(t));
        let h = head(&key).map(str::to_string);
        let same_head = |d: &Def| h.is_none() || head(&d.key).is_none() || head(&d.key) == h.as_deref();
        let by_code = code.as_ref().and_then(|c| self.list.iter().position(|d| d.codes.contains(c) && overlaps(d) && same_head(d)));
        let at = match self.by_key.get(&key).copied().or(by_code) {
            Some(i) => i,
            None => {
                if self.list.len() >= MAX_DEFS {
                    return None;
                }
                self.list.push(Def { key: key.clone(), name, trades: trades.clone(), ..Def::default() });
                let i = self.list.len() - 1;
                self.by_key.insert(key, i);
                i
            }
        };
        let def = self.list.get_mut(at)?;
        if let Some(c) = code {
            def.codes.insert(c);
        }
        if trades.is_empty() {
            def.trades.clear();
        } else if !def.trades.is_empty() {
            def.trades.extend(trades);
        }
        if def.sources.len() < MAX_SOURCES && !def.sources.iter().any(|s| s.from == src.from && s.sheet == src.sheet && s.text == src.text) {
            def.sources.push(src);
        }
        Some(at)
    }

    /// The kind best matching words `key` (its words, else one whose words it holds with the same
    /// head noun, the most words first).
    fn find(&self, key: &str) -> Option<usize> {
        if let Some(i) = self.by_key.get(key) {
            return Some(*i);
        }
        let t = tokens(key);
        let h = head(key)?;
        self.list
            .iter()
            .enumerate()
            .filter(|(_, d)| head(&d.key) == Some(h) && tokens(&d.key).is_subset(&t))
            .max_by_key(|(_, d)| tokens(&d.key).len())
            .map(|(i, _)| i)
    }
}

/// Whether `code` abbreviates `meaning`: its first letter starts one of the meaning's words and
/// its letters follow in order from there (`FSD` FIRE/SMOKE DAMPER, `SD` DUCT MOUNTED SMOKE
/// DETECTOR, `FCO` FLOOR CLEANOUT). A code with a digit is a type tag (`P-1`, `EF-2`): it names
/// no words, any meaning goes.
fn abbreviates(code: &str, meaning: &str) -> bool {
    if code.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    let letters: Vec<char> = code.chars().filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_uppercase()).collect();
    let Some((first, rest)) = letters.split_first() else { return false };
    let ws = words_of(meaning);
    if ws.first().is_some_and(|w| w.eq_ignore_ascii_case(code.trim())) {
        return false;
    }
    ws.iter().enumerate().any(|(i, w)| {
        if !w.starts_with(*first) {
            return false;
        }
        let tail: String = ws.get(i..).map(|x| x.concat()).unwrap_or_default();
        let mut it = tail.chars().skip(1);
        rest.iter().all(|c| it.any(|x| x == *c))
    })
}

/// The code–meaning pairs of the sheet's abbreviation lists and symbol legends, read row by row
/// (several pairs to a row when the list runs in columns): a code beside its meaning, or both in
/// one text; a code alone on its line takes the meaning on the line next to it that it
/// abbreviates. Every pair's code abbreviates its meaning ([`abbreviates`]). With the pairs'
/// extents.
fn abbreviations(texts: &[TextItem]) -> (Vec<(String, String)>, Vec<[f64; 4]>) {
    let d = data();
    let mark = |v: &str| super::validate_kind::looks_like_mark(v) && v.len() <= d.max_code_chars;
    let mut pairs = Vec::new();
    let mut regions = Vec::new();
    let headings = texts.iter().filter(|t| {
        let u = t.value.trim().to_ascii_uppercase();
        u.len() <= 40 && words_of(&u).iter().any(|w| listed(&d.abbreviation_headings, w))
    });
    for h in headings.take(40) {
        let rows = super::validate_kind::rows_under(h, texts);
        let mut ext: Option<[f64; 4]> = None;
        let mut grow = |t: &TextItem| {
            let r = [t.at.x, t.at.y, t.at.x + t.height * t.value.chars().count() as f64, t.at.y + t.height];
            ext = Some(match ext {
                None => r,
                Some(e) => [e[0].min(r[0]), e[1].min(r[1]), e[2].max(r[2]), e[3].max(r[3])],
            });
        };
        for (ri, row) in rows.iter().enumerate().take(2_000) {
            let mut k = 0;
            while let Some(t) = row.get(k) {
                let v = t.value.trim();
                // `FSD  FIRE/SMOKE DAMPER` in one text, or `FSD = …`, `FSD - …`.
                let one = v.split_once([' ', '=', '\t']).map(|(c, m)| (c.trim(), m.trim().trim_start_matches(['=', '-']).trim()));
                match one {
                    Some((c, m)) if mark(c) && m.len() >= 3 && abbreviates(c, m) => {
                        pairs.push((c.to_string(), m.to_string()));
                        grow(t);
                        k += 1;
                    }
                    _ if mark(v) => {
                        let beside = row.get(k + 1).filter(|n| !mark(n.value.trim()) && abbreviates(v, &n.value));
                        // Alone on its line: the next line's or the last line's meaning.
                        let near = || {
                            [ri.checked_sub(1), Some(ri + 1)]
                                .into_iter()
                                .flatten()
                                .filter_map(|j| rows.get(j))
                                .flatten()
                                .filter(|n| (n.at.y - t.at.y).abs() <= 1.6 * t.height.max(n.height))
                                .find(|n| !mark(n.value.trim()) && abbreviates(v, &n.value))
                        };
                        match beside {
                            Some(n) => {
                                pairs.push((v.to_string(), n.value.trim().to_string()));
                                grow(t);
                                grow(n);
                                k += 2;
                            }
                            None => {
                                if row.len() == 1
                                    && let Some(n) = near()
                                {
                                    pairs.push((v.to_string(), n.value.trim().to_string()));
                                    grow(t);
                                }
                                k += 1;
                            }
                        }
                    }
                    _ => k += 1,
                }
            }
        }
        if let Some(e) = ext {
            regions.push(e);
        }
    }
    (pairs, regions)
}

/// The words of a legend row beside its tag: the texts on its line right of it, left to right,
/// up to the next column (a gap of [`COLUMN_GAP_H`] text heights, or another type tag).
fn row_words(texts: &[Txt], tag: &Txt) -> String {
    let mut row: Vec<&Txt> =
        texts.iter().filter(|t| !std::ptr::eq(*t, tag) && (t.at[1] - tag.at[1]).abs() <= 0.5 * tag.h.min(t.h) && t.at[0] > tag.at[0]).collect();
    row.sort_by(|a, b| a.at[0].total_cmp(&b.at[0]));
    let mut out: Vec<&str> = Vec::new();
    let mut right = tag.at[0] + tag.h * tag.text.chars().count() as f64 * 0.8;
    for t in row {
        if t.at[0] - right > COLUMN_GAP_H * tag.h || (!out.is_empty() && cadaddy_mepfix::sheet_facts::is_type_tag(&t.text)) {
            break;
        }
        out.push(t.text.trim());
        right = t.at[0] + t.h * t.text.chars().count() as f64 * 0.8;
    }
    out.join(" ")
}

fn read_defs(sheets: &mut [Sheet], keynotes: &super::validate_keynote::Keynotes, inventory: Option<&super::inventory::Inventory>) -> Defs {
    let mut defs = Defs::default();
    let d = data();
    for s in sheets.iter_mut() {
        let items = items_of(&s.texts);
        // Legends and device schedules.
        let legend = device_types::read_legend(&items);
        s.tables.extend(legend.regions.iter().copied());
        // Each row's sample symbol: a block on the row, left of its tag (a block drawn on two
        // rows says nothing).
        let mut samples: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut words: BTreeMap<String, String> = BTreeMap::new();
        for tag in legend.rows.keys() {
            let Some(t) = s.texts.iter().find(|t| t.text.trim().eq_ignore_ascii_case(tag) && legend.covers(Vec2::new(t.at[0], t.at[1]))) else {
                continue;
            };
            words.insert(tag.clone(), row_words(&s.texts, t));
            let near = s
                .symbols
                .iter()
                .filter(|(_, p)| (p[1] - (t.at[1] + 0.5 * t.h)).abs() <= t.h && p[0] <= t.at[0] + 2.0 * t.h && p[0] >= t.at[0] - SAMPLE_REACH_H * t.h)
                .min_by(|a, b| (t.at[0] - a.1[0]).abs().total_cmp(&(t.at[0] - b.1[0]).abs()));
            if let Some((block, _)) = near {
                samples.entry(block.clone()).or_default().insert(tag.clone());
            }
        }
        for (tag, row) in &legend.rows {
            let description = words.get(tag).filter(|w| !w.is_empty()).unwrap_or(&row.description);
            let name = name_of(description);
            let key = key_of(&name);
            let from = if row.table.to_ascii_uppercase().contains("SCHEDULE") { "schedule" } else { "legend" };
            let code = tag.trim().to_ascii_uppercase();
            let src = Source { from, sheet: s.number.clone(), text: quote(&format!("{} — {} {}", row.table, tag, description)) };
            if let Some(i) = defs.add(key, name, Some(code), s.trade, src) {
                for (block, tags) in &samples {
                    if tags.len() == 1
                        && tags.contains(tag)
                        && let Some(def) = defs.list.get_mut(i)
                    {
                        def.symbols.insert(block.clone());
                    }
                }
            }
        }
        // `KEY=MEANING` designations and abbreviation lists, when the meaning names an object.
        let mut pairs: Vec<(String, String, &'static str)> =
            device_types::designations(&items).into_iter().map(|(k, v)| (k, v, "designation")).collect();
        let (abbr, regions) = abbreviations(&items);
        s.tables.extend(regions);
        pairs.extend(abbr.into_iter().map(|(k, v)| (k, v, "abbreviation")));
        for (code, meaning, from) in pairs {
            // A meaning is a few words (`FIRE DAMPER`), not a note the list runs into.
            let drafting = words_of(&meaning).iter().any(|w| listed(&d.not_object_words, &singular(w)));
            if drafting
                || !names_object(&meaning)
                || code.len() > d.max_code_chars
                || meaning.chars().count() > MAX_MEANING
                || meaning.split_whitespace().count() > 8
            {
                continue;
            }
            let name = name_of(&meaning);
            let src = Source { from, sheet: s.number.clone(), text: quote(&format!("{code} {meaning}")) };
            defs.add(key_of(&name), name, Some(code.to_ascii_uppercase()), s.trade, src);
        }
    }
    // Keynotes that provide an object (a connecting trade's note connecting another trade's
    // object, `POWER TO …`, is read as a connection).
    let connecting = |trade: &str| d.connecting_trades.iter().any(|t| t.eq_ignore_ascii_case(trade));
    for l in &keynotes.lists {
        let trade = sheets.iter().find(|s| s.number.eq_ignore_ascii_case(&l.sheet)).map_or("general", |s| s.trade);
        if let Some(s) = sheets.iter_mut().find(|s| s.number.eq_ignore_ascii_case(&l.sheet)) {
            s.tables.push(l.rect);
        }
        for n in &l.notes {
            let connects = connecting(trade) && connection_via(&n.text).is_some();
            for (key, name, after_prep, provided) in phrases(&n.text) {
                if !provided || (connects && after_prep) {
                    continue;
                }
                let src = Source { from: "keynote", sheet: l.sheet.clone(), text: quote(&format!("{}. {}", n.number, n.text)) };
                let at = defs.find(&key).or_else(|| defs.add(key.clone(), name.to_ascii_uppercase(), None, trade, src.clone()));
                if let Some(def) = at.and_then(|i| defs.list.get_mut(i)) {
                    def.keynotes.insert((l.sheet.clone(), n.number.clone()));
                    if def.sources.len() < MAX_SOURCES && !def.sources.iter().any(|x| x.text == src.text) {
                        def.sources.push(src);
                    }
                }
            }
        }
    }
    // The print inventory's kinds: its legend and schedule rows, and its object marks' kinds.
    if let Some(inv) = inventory {
        for m in inv.marks.iter().take(MAX_MARKS) {
            if !m.object && m.what != "legend_row" {
                continue;
            }
            let kind = m.kind.trim();
            if kind.is_empty() || super::validate_kind::is_generic(kind) {
                continue;
            }
            let trade = trade_of(&m.trade);
            let code = m.text.trim().to_ascii_uppercase();
            let code = (m.what == "legend_row" || m.what == "tag" || m.what == "symbol")
                .then_some(code)
                .filter(|c| super::validate_kind::looks_like_mark(c) && c.len() <= d.max_code_chars);
            // A legend row's kind on a row naming no known kind is its words; a known kind keeps
            // the row's own words as its name.
            let name = if m.what == "legend_row" && !m.source.is_empty() {
                let row = m.source.rsplit(':').next().unwrap_or(&m.source).trim();
                let row = row.strip_prefix(m.text.trim()).filter(|_| !m.text.trim().is_empty()).unwrap_or(row);
                name_of(row)
            } else {
                kind.trim_start_matches("legend:").replace('_', " ").to_ascii_uppercase()
            };
            let key = key_of(kind);
            let from = if m.what == "legend_row" { "inventory_row" } else { "inventory" };
            let text = if m.source.is_empty() { format!("{} {}", m.what, m.text) } else { m.source.clone() };
            // An object mark's kind is the inventory's; its tag only marks the kind when a row
            // names it (else every object mark would add its tag).
            let code = if m.what == "legend_row" { code } else { None };
            defs.add(key, name, code, trade, Source { from, sheet: m.sheet.clone(), text: quote(&text) });
        }
    }
    defs
}

/// The notes of the electrical, fire alarm and low-voltage sheets that connect to a kind:
/// power, a circuit, a fire alarm module, a data drop. A kind no definition names yet is defined
/// by the note (its marks: the note's keynote markers).
fn read_connections(sheets: &[Sheet], keynotes: &super::validate_keynote::Keynotes, defs: &mut Defs) {
    let d = data();
    let connecting = |trade: &str| d.connecting_trades.iter().any(|t| t.eq_ignore_ascii_case(trade));
    let mut seen: BTreeSet<(usize, String)> = BTreeSet::new();
    let mut connect = |defs: &mut Defs, sheet: &str, trade: &'static str, text: &str, keynote: Option<&str>| {
        let Some(via) = connection_via(text) else { return };
        let codes: Vec<String> = words_of(text);
        let mut hits: Vec<usize> = Vec::new();
        // Kinds named by their words after `TO`, `FOR`…; a kind of the notes' own words only so.
        for (key, name, after_prep, _) in phrases(text) {
            if head(&key).is_some_and(|h| listed(&d.own_nouns, h)) {
                continue;
            }
            let building =
                |i: usize| defs.list.get(i).is_some_and(|d| d.trades.iter().all(|t| !matches!(*t, "civil" | "landscape" | "structural" | "other")));
            match defs.find(&key).filter(|i| building(*i)) {
                Some(i) => hits.push(i),
                None if after_prep => {
                    // Another trade's object, whose trade the note doesn't say.
                    let src = Source { from: "note", sheet: sheet.to_string(), text: quote(text) };
                    if let Some(i) = defs.add(key, name.to_ascii_uppercase(), None, "general", src) {
                        hits.push(i);
                    }
                }
                None => {}
            }
        }
        // Kinds named by a code (`POWER TO FSD`): another trade's codes of two letters or more.
        for (i, def) in defs.list.iter().enumerate() {
            let building = def.trades.iter().all(|t| !matches!(*t, "civil" | "landscape" | "structural" | "other"));
            if building && def.codes.iter().any(|c| c.len() >= 3 && codes.iter().any(|w| w == c)) && !def.trades.contains(trade) {
                hits.push(i);
            }
        }
        for i in hits {
            // A kind of the connecting trade's own (its own module, its own box) is no other trade's.
            let Some(def) = defs.list.get_mut(i) else { continue };
            if !def.trades.is_empty() && def.trades.iter().all(|t| *t == trade) && def.sources.iter().all(|s| s.from != "note") {
                continue;
            }
            if let Some(k) = keynote {
                def.keynotes.insert((sheet.to_string(), k.to_string()));
            }
            if seen.insert((i, sheet.to_ascii_uppercase())) && def.connected.len() < MAX_LISTED {
                def.connected.push((sheet.to_string(), via, quote(text)));
            }
        }
    };
    for l in &keynotes.lists {
        let Some(s) = sheets.iter().find(|s| s.number.eq_ignore_ascii_case(&l.sheet)) else { continue };
        if !connecting(s.trade) {
            continue;
        }
        for n in &l.notes {
            connect(defs, &s.number, s.trade, &n.text, Some(&n.number));
        }
    }
    for s in sheets.iter().filter(|s| connecting(s.trade)) {
        for (text, at) in &s.notes {
            if s.tables.iter().any(|r| inside(*r, *at)) {
                continue;
            }
            connect(defs, &s.number, s.trade, text, None);
        }
    }
}

// ---------------------------------------------------------------- the marks

/// One mark of a kind on a plan.
#[derive(Clone, Debug)]
struct PlanMark {
    def: usize,
    sheet: String,
    sheet_at: [f64; 2],
    how: &'static str,
    /// Its level and plan point (feet), when its sheet is placed; a typical-floor view's every level.
    placed: Vec<(String, [f64; 2])>,
    /// The unit type whose enlarged plan view holds it, and that view.
    typical: Option<(String, usize)>,
    /// Why it couldn't be placed.
    unplaced: Option<&'static str>,
}

/// A unit type's enlarged plan views: (type id, [(sheet, title, rect)]), and how many rooms are
/// of it.
struct Typicals {
    types: Vec<(String, Vec<(String, String, [f64; 4])>)>,
    rooms: HashMap<String, usize>,
}

impl Typicals {
    fn of(sp: &BuildingSpec) -> Typicals {
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let mut types = Vec::new();
        for t in sp.unit_types.as_array().into_iter().flatten().take(1_000) {
            let id = Some(text(t, "id")).filter(|x| !x.is_empty()).unwrap_or_else(|| text(t, "tag"));
            if id.is_empty() {
                continue;
            }
            let mut views = Vec::new();
            for v in t.get("views").and_then(Value::as_array).into_iter().flatten().take(40) {
                let (sheet, title) = (text(v, "sheet"), text(v, "title"));
                let up = title.to_ascii_uppercase();
                if sheet.is_empty() || [" WALL", "ELEVATION", "SECTION", "DETAIL"].iter().any(|w| up.contains(w)) {
                    continue;
                }
                let rect = |x: Option<&Value>| -> Option<[f64; 4]> {
                    let a = x?.as_array()?;
                    let r = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?, a.get(3)?.as_f64()?];
                    r.iter().all(|v| v.is_finite()).then(|| norm_rect(r))
                };
                if let Some(r) = rect(v.get("rect")) {
                    views.push((sheet, title, r));
                }
            }
            if !views.is_empty() {
                types.push((id, views));
            }
        }
        let mut rooms: HashMap<String, usize> = HashMap::new();
        for i in sp.unit_instances.as_array().into_iter().flatten().take(100_000) {
            let id = Some(text(i, "type")).filter(|x| !x.is_empty()).unwrap_or_else(|| text(i, "unit_type"));
            if !id.is_empty() {
                *rooms.entry(id.to_ascii_uppercase()).or_default() += 1;
            }
        }
        Typicals { types, rooms }
    }

    fn view_of(&self, sheet: &str, p: [f64; 2]) -> Option<(String, usize)> {
        for (id, views) in &self.types {
            if let Some(k) = views.iter().position(|(s, _, r)| s.eq_ignore_ascii_case(sheet) && inside(*r, p)) {
                return Some((id.clone(), k));
            }
        }
        None
    }

    fn rooms_of(&self, id: &str) -> usize {
        self.rooms.get(&id.to_ascii_uppercase()).copied().unwrap_or(0)
    }
}

/// Where a sheet point lies on its level's plan: every level it shows (feet), else why not.
fn place(sp: &BuildingSpec, frame: &Frame, sheet: &str, p: [f64; 2]) -> Result<Vec<(String, [f64; 2])>, &'static str> {
    let links: Vec<&SheetLink> = sp.sheets.iter().filter(|l| l.number.eq_ignore_ascii_case(sheet)).collect();
    let in_view = links.iter().copied().find(|l| l.view.as_ref().is_some_and(|v| inside(norm_rect(v.rect), p)));
    let link = in_view.or_else(|| links.iter().copied().find(|l| l.view.is_none())).ok_or("in no view of its sheet that shows a level")?;
    let level = link.level.clone().filter(|l| !l.trim().is_empty()).ok_or("its sheet shows no level")?;
    let project = match &link.registration {
        Some(r) => r.to_project(p),
        None if sp.level(&level).and_then(|l| l.sheet.as_deref()).is_some_and(|sh| sh.eq_ignore_ascii_case(sheet))
            && frame.to_ft(Vec2::new(p[0], p[1])).0.eq_ignore_ascii_case(&level) =>
        {
            p
        }
        None => return Err("its sheet is not placed on its level's plan (bim.identity registers it)"),
    };
    if !project.iter().all(|v| v.is_finite()) {
        return Err("its registration gives no point");
    }
    let at = frame.to_ft_on(&level, Vec2::new(project[0], project[1]));
    let mut out = vec![(level.clone(), at)];
    for l in &link.levels {
        if !out.iter().any(|(x, _)| x.eq_ignore_ascii_case(l)) {
            out.push((l.clone(), at));
        }
    }
    Ok(out)
}

fn read_marks(
    sp: &BuildingSpec,
    frame: &Frame,
    sheets: &[Sheet],
    defs: &Defs,
    keynotes: &super::validate_keynote::Keynotes,
    typ: &Typicals,
) -> Vec<PlanMark> {
    let mut by_code: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_symbol: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, def) in defs.list.iter().enumerate() {
        for c in &def.codes {
            by_code.entry(c.as_str()).or_default().push(i);
        }
        for b in &def.symbols {
            by_symbol.entry(b.as_str()).or_default().push(i);
        }
    }
    let shows = |def: &Def, trade: &str| def.trades.is_empty() || trade == "general" || def.trades.contains(trade);
    let mut out: Vec<PlanMark> = Vec::new();
    let push = |out: &mut Vec<PlanMark>, def: usize, sheet: &Sheet, p: [f64; 2], how: &'static str| {
        if out.len() >= MAX_MARKS {
            return;
        }
        let typical = typ.view_of(&sheet.number, p);
        let (placed, unplaced) = match place(sp, frame, &sheet.number, p) {
            Ok(v) => (v, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        out.push(PlanMark { def, sheet: sheet.number.clone(), sheet_at: [r2(p[0]), r2(p[1])], how, placed, typical, unplaced });
    };
    for s in sheets.iter().filter(|s| s.plan) {
        let mut here: Vec<(usize, [f64; 2], f64)> = Vec::new();
        for t in &s.texts {
            // Its tables' lettering, and its own number (the title block's), are no tags.
            if s.tables.iter().any(|r| inside(*r, t.at)) || t.text.trim().eq_ignore_ascii_case(&s.number) {
                continue;
            }
            let mut hit: BTreeSet<usize> = BTreeSet::new();
            for c in tag_codes(&t.text) {
                // A lone letter is as often a stroke, a grid or a note's letter as a tag.
                if c.chars().count() < 2 {
                    continue;
                }
                for i in by_code.get(c.as_str()).into_iter().flatten() {
                    if defs.list.get(*i).is_some_and(|d| shows(d, s.trade)) {
                        hit.insert(*i);
                    }
                }
                if !hit.is_empty() {
                    break;
                }
            }
            if hit.iter().any(|i| defs.list.get(*i).is_some_and(|d| d.trades.contains(s.trade))) {
                hit.retain(|i| defs.list.get(*i).is_some_and(|d| d.trades.contains(s.trade)));
            }
            for i in hit {
                // The same tag twice in one place (a text and its block's attribute) is one.
                if here.iter().any(|(j, p, h)| *j == i && (p[0] - t.at[0]).hypot(p[1] - t.at[1]) <= SAME_TAG_H * same_reach(*h, t.h)) {
                    continue;
                }
                here.push((i, t.at, t.h));
                push(&mut out, i, s, t.at, "tag");
            }
        }
        for (block, p) in &s.symbols {
            if s.tables.iter().any(|r| inside(*r, *p)) {
                continue;
            }
            for i in by_symbol.get(block.as_str()).into_iter().flatten() {
                if !defs.list.get(*i).is_some_and(|d| shows(d, s.trade)) {
                    continue;
                }
                // A symbol with its tag beside it is one mark.
                if here.iter().any(|(j, q, h)| *j == *i && (q[0] - p[0]).hypot(q[1] - p[1]) <= 4.0 * h) {
                    continue;
                }
                here.push((*i, *p, 1.0));
                push(&mut out, *i, s, *p, "symbol");
            }
        }
    }
    // Keynote markers of the notes that call for a kind.
    let mut keyed: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, def) in defs.list.iter().enumerate() {
        for (sheet, n) in &def.keynotes {
            keyed.entry((sheet.to_ascii_uppercase(), n.clone())).or_default().push(i);
        }
    }
    // A marker carried onto several levels (a typical-floor view) is one mark on each.
    let mut done: HashMap<(usize, String, [i64; 2]), usize> = HashMap::new();
    for m in &keynotes.markers {
        for i in keyed.get(&(m.sheet.to_ascii_uppercase(), m.number.clone())).into_iter().flatten() {
            let k = (*i, m.sheet.to_ascii_uppercase(), [(m.sheet_at[0] * 10.0).round() as i64, (m.sheet_at[1] * 10.0).round() as i64]);
            if out.len() >= MAX_MARKS {
                break;
            }
            let typical = typ.view_of(&m.sheet, m.sheet_at);
            match done.get(&k).and_then(|j| out.get_mut(*j)) {
                Some(x) => {
                    if !x.placed.iter().any(|(l, _)| l.eq_ignore_ascii_case(&m.level)) {
                        x.placed.push((m.level.clone(), m.at));
                    }
                }
                None => {
                    done.insert(k, out.len());
                    out.push(PlanMark {
                        def: *i,
                        sheet: m.sheet.clone(),
                        sheet_at: m.sheet_at,
                        how: "keynote",
                        placed: vec![(m.level.clone(), m.at)],
                        typical,
                        unplaced: None,
                    });
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the model

/// A model element as the prescreen reads it.
#[derive(Clone, Debug)]
pub(crate) struct Census {
    pub handle: String,
    pub kind: String,
    pub mark: Option<String>,
    pub name: Option<String>,
    pub level: String,
    /// Where it was traced from (`E-201 @ 12'-3", 40'-0"`): its sheet's trade guards a tag match.
    pub source: Option<String>,
}

/// How well an element matches a kind: 5 its mark is one of the kind's codes, 4 its mark is a
/// code numbered (`FSD-3`) or its kind's words are the kind's, 3 its type's words are, 2 it holds
/// the kind's words with the same head noun, 1 its kind's words are among the kind's; 0 no match.
fn score(def: &Def, el: &Census, el_key: &str, name_key: &str, el_trade: Option<&str>) -> u8 {
    // A tag is the kind's only on its own trade's sheets (`SD` a duct detector on the mechanical
    // plans, a smoke detector on the electrical ones).
    let own = def.trades.is_empty() || el_trade.is_none_or(|t| t == "general" || def.trades.contains(t));
    if let Some(m) = el.mark.as_deref().map(|m| m.trim().to_ascii_uppercase()).filter(|m| !m.is_empty() && own) {
        if def.codes.contains(&m) {
            return 5;
        }
        if tag_codes(&m).iter().skip(1).any(|c| def.codes.contains(c)) {
            return 4;
        }
    }
    if !el_key.is_empty() && el_key == def.key {
        return 4;
    }
    if !name_key.is_empty() && name_key == def.key {
        return 3;
    }
    let dk = tokens(&def.key);
    let Some(h) = head(&def.key) else { return 0 };
    let mut have: BTreeSet<&str> = tokens(el_key);
    have.extend(tokens(name_key));
    if dk.is_subset(&have) && (head(el_key) == Some(h) || head(name_key) == Some(h)) {
        return 2;
    }
    // Its kind's words (two or more) inside the kind's longer words, with the same head noun
    // (`occupancy_sensor` for `FULL AUTOMATIC OCCUPANCY SENSOR`): the weakest match, taken only
    // when no kind matches better.
    let ek = tokens(el_key);
    if ek.len() >= 2 && ek.is_subset(&dk) && head(el_key) == Some(h) {
        return 1;
    }
    0
}

/// Whether an element carries one of the kind's tags as its mark (`FSD`, `FSD-3`).
fn by_mark(def: &Def, el: &Census, el_trade: Option<&str>) -> bool {
    let own = def.trades.is_empty() || el_trade.is_none_or(|t| t == "general" || def.trades.contains(t));
    own && el.mark.as_deref().map(|m| m.trim().to_ascii_uppercase()).is_some_and(|m| tag_codes(&m).iter().any(|c| def.codes.contains(c)))
}

// ---------------------------------------------------------------- checking

/// What G-COVERAGE found: its findings (rank, verdict, finding), what it couldn't measure (by
/// why) and its table, kind by kind.
#[derive(Default)]
pub(crate) struct Got {
    pub findings: Vec<(u8, &'static str, Value)>,
    /// The sheets the drawing doesn't hold whose pages were read for their definitions.
    pub pages_read: Vec<String>,
    pub unmeasured: BTreeMap<String, u64>,
    pub table: Value,
}

/// What G-COVERAGE reads from.
pub(crate) struct Input<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub frame: &'a Frame,
    pub areas: &'a [Area],
    pub whole_levels: bool,
    pub census: &'a [Census],
}

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(default)
}

fn in_area(a: &Area, p: [f64; 2]) -> bool {
    inside(a.bbox, p) && (a.outline.len() < 3 || point_in(&a.outline, p))
}

/// The kind of object in words, for an issue (`FIRE/SMOKE DAMPER (FSD)`).
fn label(def: &Def) -> String {
    let codes: Vec<&str> = def.codes.iter().map(String::as_str).take(4).collect();
    let name = if def.name.is_empty() { def.key.to_ascii_uppercase() } else { def.name.clone() };
    if codes.is_empty() || codes.iter().any(|c| name.eq_ignore_ascii_case(c)) { name } else { format!("{name} ({})", codes.join(", ")) }
}

fn sheets_words(by_sheet: &BTreeMap<String, usize>) -> String {
    let list: Vec<String> = by_sheet.iter().take(MAX_LISTED).map(|(s, n)| format!("{s} ({n})")).collect();
    let more = by_sheet.len().saturating_sub(MAX_LISTED);
    if more > 0 { format!("{} and {more} more sheets", list.join(", ")) } else { list.join(", ") }
}

pub(crate) fn check(inp: &Input, r: &Effective) -> Got {
    let mut got = Got::default();
    let (share, abs) = (tol(r, "count_share", DEFAULT_SHARE), tol(r, "count_abs", DEFAULT_ABS));
    let dedupe = tol(r, "dedupe_ft", DEFAULT_DEDUPE_FT).max(0.01);
    let (repeat_sheets, repeat_marks) =
        (tol(r, "repeat_sheets", DEFAULT_REPEAT_SHEETS).max(1.0) as usize, tol(r, "repeat_marks", DEFAULT_REPEAT_MARKS).max(1.0) as usize);
    let levels: BTreeSet<String> = inp.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    // The whole building: every level that has rooms is in scope.
    let all_levels =
        inp.whole_levels && super::validate_packet::model_areas(inp.d, inp.frame).iter().all(|a| levels.contains(&a.level.to_ascii_lowercase()));
    let on_scope = |level: &str| levels.contains(&level.to_ascii_lowercase());
    // A level as the inventory names it (its name, or its number: `1` for `Level 1`).
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let level_digits: BTreeSet<String> = levels.iter().map(|l| digits(l)).filter(|d| !d.is_empty()).collect();
    let on_scope_inv = |l: &str| on_scope(l) || (!digits(l).is_empty() && level_digits.contains(&digits(l)));

    let mut sheets = read_sheets(inp.d, inp.sp);
    // The legend, schedule and abbreviation sheets the drawing doesn't hold, off their pages.
    if !sheets.is_empty() {
        let loaded: BTreeSet<String> = sheets.iter().map(|s| s.number.split('/').next().unwrap_or("").trim().to_ascii_uppercase()).collect();
        let pages = read_pages(inp.d, &loaded, &mut got.unmeasured);
        got.pages_read = pages.iter().map(|p| p.number.clone()).collect();
        sheets.extend(pages);
    }
    if sheets.is_empty() {
        *got.unmeasured.entry("the drawing holds no print sheets (sheet links with a place on the drawing)".into()).or_default() += 1;
        got.table = json!({"kinds": [], "unexplained_groups": [], "source": data().source});
        return got;
    }
    let keynotes = super::validate_keynote::Keynotes::read(inp.d, inp.sp, inp.frame);
    let inventory = super::inventory::stored(inp.d);
    let mut defs = read_defs(&mut sheets, &keynotes, inventory.as_ref());
    read_connections(&sheets, &keynotes, &mut defs);
    let typ = Typicals::of(inp.sp);
    let marks = read_marks(inp.sp, inp.frame, &sheets, &defs, &keynotes, &typ);
    let units = super::validate_prescreen::Units::of(inp.d);

    // The model's elements, each to the kinds it matches best.
    let sheet_trade: HashMap<String, &'static str> = sheets.iter().map(|s| (s.number.to_ascii_uppercase(), s.trade)).collect();
    let trade_of_el = |el: &Census| -> Option<&'static str> {
        let src = el.source.as_deref()?.split_whitespace().next()?.trim_end_matches([',', ';', ':']).to_ascii_uppercase();
        sheet_trade.get(&src).copied()
    };
    // Else its kind's trade (a smoke detector is electrical): it guards a building trade's
    // kinds only (a sheet letter like `L` means another trade from set to set).
    let trade_of_el = |el: &Census, def: &Def| -> Option<&'static str> {
        trade_of_el(el).or_else(|| {
            let building = def.trades.iter().all(|t| !matches!(*t, "civil" | "landscape" | "structural" | "other"));
            Some(super::validate_packet::trade_of_kind(&el.kind, &[])).filter(|t| *t != "general" && building)
        })
    };
    let mut model: Vec<Vec<&Census>> = defs.list.iter().map(|_| Vec::new()).collect();
    // Each kind's elements by any match, the best or not: a kind with no element of its own may
    // share another kind's (two rows naming one kind).
    let mut any: Vec<Vec<&Census>> = defs.list.iter().map(|_| Vec::new()).collect();
    for el in inp.census.iter().filter(|e| on_scope(&e.level)) {
        let ek = key_of(&el.kind);
        let nk = el.name.as_deref().map(key_of).unwrap_or_default();
        let scores: Vec<u8> = defs.list.iter().map(|d| score(d, el, &ek, &nk, trade_of_el(el, d))).collect();
        for (i, s) in scores.iter().enumerate() {
            if *s >= 2
                && let Some(v) = any.get_mut(i)
                && v.len() < MAX_SHARED
            {
                v.push(el);
            }
        }
        let best = scores.iter().copied().max().unwrap_or(0);
        if best < 1 {
            continue;
        }
        for (i, s) in scores.iter().enumerate() {
            if *s == best
                && let Some(v) = model.get_mut(i)
            {
                v.push(el);
            }
        }
    }

    // Each kind's marks: raw by sheet, the typical plans' per room, the plans' deduped by level.
    struct Count {
        by_sheet: BTreeMap<String, usize>,
        by_level: BTreeMap<String, usize>,
        typical: BTreeMap<String, BTreeMap<usize, usize>>,
        unplaced: BTreeMap<String, usize>,
        placed: Vec<(String, String, [f64; 2], [f64; 2])>,
        how: BTreeMap<&'static str, usize>,
    }
    let mut counts: Vec<Count> = defs
        .list
        .iter()
        .map(|_| Count {
            by_sheet: BTreeMap::new(),
            by_level: BTreeMap::new(),
            typical: BTreeMap::new(),
            unplaced: BTreeMap::new(),
            placed: Vec::new(),
            how: BTreeMap::new(),
        })
        .collect();
    for m in &marks {
        let Some(c) = counts.get_mut(m.def) else { continue };
        let in_scope = if m.placed.is_empty() { all_levels } else { m.placed.iter().any(|(l, _)| on_scope(l)) };
        if !in_scope {
            continue;
        }
        *c.by_sheet.entry(m.sheet.clone()).or_default() += 1;
        *c.how.entry(m.how).or_default() += 1;
        if let Some((ty, view)) = &m.typical {
            *c.typical.entry(ty.clone()).or_default().entry(*view).or_default() += 1;
            continue;
        }
        if m.placed.is_empty() {
            *c.unplaced.entry(m.unplaced.unwrap_or("not placed").to_string()).or_default() += 1;
            continue;
        }
        for (l, at) in m.placed.iter().filter(|(l, _)| on_scope(l)) {
            c.placed.push((l.clone(), m.sheet.clone(), *at, m.sheet_at));
        }
    }
    let typ_areas: Vec<(&Area, String)> =
        inp.areas.iter().filter_map(|a| units.get(&a.level, a.number.as_deref()).map(|u| (a, u.id.to_ascii_uppercase()))).collect();
    let mut rows: Vec<(u8, Value)> = Vec::new();
    let mut not_marked = 0u64;
    for (i, def) in defs.list.iter().enumerate() {
        let Some(c) = counts.get_mut(i) else { continue };
        // A floor plan's mark in a room of a unit type whose plan shows this kind is that plan's.
        let shown: BTreeSet<String> = c.typical.keys().map(|t| t.to_ascii_uppercase()).collect();
        c.placed.retain(|(l, _, at, _)| !typ_areas.iter().any(|(a, ty)| shown.contains(ty) && a.level.eq_ignore_ascii_case(l) && in_area(a, *at)));
        // The same kind within `dedupe` on another sheet of the level is the same object.
        let mut kept: Vec<(String, String, [f64; 2], [f64; 2])> = Vec::new();
        let mut placed = std::mem::take(&mut c.placed);
        placed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for p in placed {
            let dup = kept.iter().any(|k| k.0.eq_ignore_ascii_case(&p.0) && k.1 != p.1 && (k.2[0] - p.2[0]).hypot(k.2[1] - p.2[1]) <= dedupe);
            if !dup {
                *c.by_level.entry(p.0.clone()).or_default() += 1;
                kept.push(p);
            }
        }
        let mut typical_json = Vec::new();
        let mut typical_n = 0usize;
        for (ty, views) in &c.typical {
            let per_room = views.values().copied().max().unwrap_or(0);
            let rooms = typ.rooms_of(ty);
            typical_n += per_room * rooms.max(1);
            typical_json.push(json!({"unit_type": ty, "per_room": per_room, "rooms": rooms, "views": views.len()}));
        }
        let unplaced_n: usize = c.unplaced.values().sum();
        let expected = kept.len() + typical_n + unplaced_n;
        let own_elements = model.get(i).map_or(&[][..], Vec::as_slice);
        let shared = own_elements.is_empty() && any.get(i).is_some_and(|v| !v.is_empty());
        let elements = if shared { any.get(i).map_or(&[][..], Vec::as_slice) } else { own_elements };
        let m_n = elements.len();
        let connected = !def.connected.is_empty();
        let raw: usize = c.by_sheet.values().sum();
        // In an area or room scope only the kinds marked inside its areas are its.
        let areas_marked: Vec<&Area> = if inp.whole_levels {
            Vec::new()
        } else {
            inp.areas.iter().filter(|a| kept.iter().any(|k| k.0.eq_ignore_ascii_case(&a.level) && in_area(a, k.2))).collect()
        };
        if !inp.whole_levels && areas_marked.is_empty() {
            continue;
        }
        let (verdict, rank, severity) = if expected == 0 && !connected {
            not_marked += 1;
            ("not_marked", 5u8, "low")
        } else if m_n == 0 {
            ("fail", if connected { 0 } else { 1 }, "high")
        } else {
            // Fewer than the plans show is a fail; more only when the elements carry the kind's
            // own tag (an element matched by its kind's words may be one the plans mark otherwise,
            // and a keynote marks only the ones it calls for).
            let slack = abs.max((share * expected as f64).round());
            let marked = elements.iter().filter(|e| by_mark(def, e, trade_of_el(e, def))).count();
            let under = expected as f64 - m_n as f64 > slack;
            let over = !def.codes.is_empty() && marked as f64 - expected as f64 > slack;
            if expected > 0 && (under || over) { ("fail", 2, "medium") } else { ("pass", 4, "low") }
        };
        let sheets_said = sheets_words(&c.by_sheet);
        let name = label(def);
        let via: BTreeSet<&str> = def.connected.iter().map(|(_, v, _)| *v).collect();
        let connected_sheets: BTreeSet<&str> = def.connected.iter().map(|(s, _, _)| s.as_str()).collect();
        let connect_words = || {
            let v: Vec<&str> = via.iter().map(|v| v.trim_end_matches("_alarm")).collect();
            format!(
                "; {} connect{} to it ({})",
                connected_sheets.iter().copied().take(6).collect::<Vec<_>>().join(", "),
                if connected_sheets.len() == 1 { "s" } else { "" },
                v.iter().map(|x| if *x == "fire" { "fire alarm" } else { x }).collect::<Vec<_>>().join(", ")
            )
        };
        let issue = match (verdict, m_n) {
            ("fail", 0) if expected == 0 => format!("{name}: the electrical sheets connect to it, 0 in the model{}", connect_words()),
            ("fail", 0) => {
                format!("{name}: {expected} marks on {sheets_said}, 0 in the model{}", if connected { connect_words() } else { String::new() })
            }
            ("fail", _) => format!("{name}: {expected} marks on the plans ({sheets_said}), {m_n} in the model"),
            ("pass", _) => format!("{name}: {expected} marks on the plans, {m_n} in the model"),
            _ => format!("{name}: defined, no mark on a plan in scope"),
        };
        let defined_by: Vec<Value> = def.sources.iter().map(|s| json!({"from": s.from, "sheet": s.sheet, "text": s.text})).collect();
        let row = json!({
            "kind": def.key,
            "name": name,
            "codes": def.codes.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "symbols": def.symbols.len(),
            "trades": def.trades.iter().collect::<Vec<_>>(),
            "defined_by": defined_by,
            "plan_marks": expected,
            "raw_marks": raw,
            "by_sheet": c.by_sheet,
            "by_level": c.by_level,
            "by_how": c.how,
            "typical": typical_json,
            "unplaced": c.unplaced,
            "model": m_n,
            "model_by_tag": elements.iter().filter(|e| by_mark(def, e, trade_of_el(e, def))).count(),
            "model_shared": shared,
            "model_elements": elements.iter().take(MAX_LISTED).map(|e| e.handle.clone()).collect::<Vec<_>>(),
            "model_kinds": elements.iter().map(|e| e.kind.as_str()).collect::<BTreeSet<_>>().into_iter().take(6).collect::<Vec<_>>(),
            "connected": def.connected.iter().map(|(s, v, t)| json!({"sheet": s, "via": v, "text": t})).collect::<Vec<_>>(),
            "verdict": verdict,
        });
        let table_row = if verdict == "not_marked" {
            json!({"kind": def.key, "name": name, "codes": row["codes"], "defined_by": defined_by.iter().take(2).collect::<Vec<_>>(), "plan_marks": 0, "model": m_n, "verdict": verdict})
        } else {
            row.clone()
        };
        rows.push((rank, table_row));
        if verdict != "not_marked" {
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|{}|{verdict}", def.key)));
            let weak = def.symbols.is_empty() && def.keynotes.is_empty() && def.codes.iter().all(|c| c.chars().count() < 2);
            let first = kept.first();
            let area = areas_marked.first().map(|a| a.id.clone());
            let level = first.map(|k| k.0.clone()).or_else(|| c.by_level.keys().next().cloned());
            let expected_words = if expected == 0 {
                format!("the {name} the electrical sheets connect to, in the model")
            } else {
                format!("{expected} {name} (the plans' marks, deduped by registration)")
            };
            got.findings.push((
                rank,
                verdict_static(verdict),
                json!({
                    "id": id,
                    "rule": RULE,
                    "kind": def.key,
                    "verdict": verdict,
                    "issue": issue,
                    "expected": expected_words,
                    "found": format!("{m_n} in the model"),
                    "measured": row,
                    "evidence": [],
                    "confidence": if weak { 0.6 } else { 0.9 },
                    "severity": if verdict == "pass" { r.rule.severity.as_str() } else { severity },
                    "area": area,
                    "level": level,
                    "at": first.map(|k| pt(k.2)),
                    "source": "prescreen",
                }),
            ));
        }
    }
    if not_marked > 0 {
        got.unmeasured.insert("kinds the documents define that no plan in scope marks".into(), not_marked);
    }

    // Repeated unexplained marks: a kind the documents never define.
    let mut groups_json = Vec::new();
    match &inventory {
        None => {
            got.unmeasured.insert("no print inventory in the drawing (the job's BIM build takes it): unexplained marks not grouped".into(), 1);
        }
        Some(inv) => {
            let codes: BTreeSet<&str> = defs.list.iter().flat_map(|d| d.codes.iter().map(String::as_str)).collect();
            let mut groups: BTreeMap<(String, String), (BTreeSet<String>, usize, Vec<Value>)> = BTreeMap::new();
            for m in inv.marks.iter().take(MAX_MARKS) {
                if m.status != super::inventory::UNEXPLAINED
                    || m.level.trim().is_empty()
                    || !matches!(m.what.as_str(), "tag" | "symbol" | "text" | "block")
                {
                    continue;
                }
                let in_scope = all_levels || m.level.split(',').any(|l| on_scope_inv(l.trim()));
                if !in_scope {
                    continue;
                }
                let text = m.text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
                if text.is_empty() || text.chars().count() > 20 || !text.chars().any(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                if tag_codes(&text).iter().any(|c| codes.contains(c.as_str())) {
                    continue;
                }
                let g = groups.entry((m.what.clone(), text)).or_insert_with(|| (BTreeSet::new(), 0, Vec::new()));
                g.0.insert(m.sheet.clone());
                g.1 += m.count.max(1);
                if g.2.len() < MAX_EXAMPLES {
                    g.2.push(json!({"sheet": m.sheet, "at": pt(m.at), "text": m.text, "level": m.level}));
                }
            }
            let mut list: Vec<((String, String), (BTreeSet<String>, usize, Vec<Value>))> =
                groups.into_iter().filter(|(_, g)| g.0.len() >= repeat_sheets && g.1 >= repeat_marks).collect();
            list.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(b.1.0.len().cmp(&a.1.0.len())).then(a.0.cmp(&b.0)));
            let total = list.len();
            for ((what, text), (sheets_on, n, examples)) in list.into_iter().take(data().max_groups) {
                let issue = format!(
                    "'{text}' ({what}) printed {n} times on {} plan sheets ({}), and nothing the documents define says what it is: likely a kind of object missing from the model",
                    sheets_on.len(),
                    sheets_on.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
                );
                let measured = json!({"what": what, "text": text, "marks": n, "sheets": sheets_on.iter().take(MAX_LISTED).collect::<Vec<_>>(), "examples": examples});
                groups_json.push(measured.clone());
                if !inp.whole_levels {
                    continue;
                }
                got.findings.push((
                    3,
                    "unsure",
                    json!({
                        "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|unexplained|{what}|{text}"))),
                        "rule": RULE,
                        "verdict": "unsure",
                        "issue": issue,
                        "expected": "a legend, schedule or abbreviation row naming it, and its elements in the model (or a reader rule saying it is no object)",
                        "found": format!("{n} unexplained marks, no kind"),
                        "measured": measured,
                        "evidence": [],
                        "confidence": 0.5,
                        "severity": "medium",
                        "area": Value::Null,
                        "level": examples.first().and_then(|e| e["level"].as_str()).map(|l| l.split(',').next().unwrap_or(l).to_string()),
                        "source": "prescreen",
                    }),
                ));
            }
            if total > groups_json.len() {
                got.unmeasured.insert("repeated unexplained mark groups past the rules data's max_groups".into(), (total - groups_json.len()) as u64);
            }
        }
    }
    rows.sort_by_key(|r| r.0);
    let kinds: Vec<Value> = rows.into_iter().map(|(_, v)| v).take(MAX_ROWS).collect();
    got.table = json!({
        "about": "every kind the documents define (legend and schedule rows, abbreviations, keynotes, notes, the print inventory's kinds): its plan marks (per sheet and level, a floor plan and an enlarged plan deduped by registration, a typical unit plan once per room of its type), the model's elements of that kind and the verdict; zero-element kinds first",
        "kinds": kinds,
        "kinds_defined": defs.list.len(),
        "pages_read": got.pages_read,
        "unexplained_groups": groups_json,
        "source": data().source,
    });
    got.findings.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2["id"].as_str().unwrap_or_default().cmp(b.2["id"].as_str().unwrap_or_default())));
    got
}

fn verdict_static(v: &str) -> &'static str {
    VERDICTS.iter().copied().find(|x| *x == v).unwrap_or("unsure")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_key_on_their_words() {
        assert_eq!(key_of("FIRE/SMOKE DAMPERS"), "fire smoke damper");
        assert_eq!(key_of("fire_smoke_damper"), "fire smoke damper");
        assert_eq!(key_of("legend:night glow marker 24v"), "night glow marker");
        assert_eq!(name_of("DUPLEX RECEPTACLE, 20A, 125V"), "DUPLEX RECEPTACLE");
        assert!(names_object("COMBINATION FIRE SMOKE DAMPER"));
        assert!(!names_object("ABOVE FINISHED FLOOR"));
    }

    #[test]
    fn a_note_names_its_objects() {
        let p = phrases("PROVIDE 120V POWER TO THE FIRE/SMOKE DAMPERS ABOVE THE CEILING.");
        assert!(p.iter().any(|(k, _, prep, _)| k == "fire smoke damper" && !*prep), "{p:?}");
        let p = phrases("CONNECT TO MOTORIZED DAMPER");
        assert!(p.iter().any(|(k, _, prep, provided)| k == "motorized damper" && *prep && !*provided), "{p:?}");
        let p = phrases("PROVIDE DUCT SMOKE DETECTOR RELAY.");
        assert_eq!(p.len(), 1, "{p:?}");
        let p = phrases("EXISTING EXHAUST FAN TO REMAIN");
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(connection_via("CONNECT TO DEDICATED CIRCUIT X-9"), Some("power"));
        assert_eq!(connection_via("PROVIDE CONTROL MODULE"), Some("fire_alarm"));
        assert_eq!(connection_via("SEE ARCHITECTURAL"), None);
    }

    #[test]
    fn a_tag_prints_its_code_and_its_number() {
        assert_eq!(tag_codes("FSD-3"), vec!["FSD-3".to_string(), "FSD".to_string()]);
        assert_eq!(tag_codes("(MD)"), vec!["MD".to_string()]);
        assert_eq!(tag_codes("S3"), vec!["S3".to_string()]);
        assert!(tag_codes("PROVIDE A NEW DAMPER HERE").is_empty());
    }
}
