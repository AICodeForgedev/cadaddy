//! SPELL: check the spelling of text, multiline text, attribute values, dimension text overrides,
//! table cells and multileader text against an English word list (derived from SCOWL / the
//! English Speller Database, `assets/spell/en_US.txt`) and a user dictionary, and change words,
//! one or all of them, ignore words or add them to the user dictionary.
//!
//! DCTMAIN names the main dictionary (`enu`, American English, the one that ships); DCTCUST is
//! the user dictionary file (one word per line), by default `custom.cus` in the CADaddy settings
//! folder. Words with digits are skipped; MTEXT format codes, `%%` codes and fields are not text.
//! A word matches as written, in lower case, or with a capital first letter, so `BRACKET` and
//! `Bracket` are both right when `bracket` is in the list.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use cadaddy_doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// The word list, with its notice before the `---` line.
const LIST: &str = include_str!("../../../../assets/spell/en_US.txt");
/// The main dictionary's code (DCTMAIN).
pub const MAIN_CODE: &str = "enu";
/// Most suggestions offered.
const MAX_SUGGESTIONS: usize = 8;
/// Most misspellings one check reports.
const MAX_FOUND: usize = 10_000;
/// Longest word checked (longer runs are not words).
const MAX_WORD: usize = 64;
/// Largest user dictionary read.
const MAX_CUSTOM_BYTES: u64 = 8 * 1024 * 1024;

fn main_words() -> &'static HashSet<&'static str> {
    static MAIN: OnceLock<HashSet<&'static str>> = OnceLock::new();
    MAIN.get_or_init(|| {
        let body = LIST.split_once("\n---\n").map_or(LIST, |(_, w)| w);
        body.lines().map(str::trim).filter(|w| !w.is_empty()).collect()
    })
}

/// The user dictionary path, its words, and the words ignored for this session (Ignore All).
#[derive(Default)]
struct User {
    path: Option<String>,
    /// The words of `path`, read on first use (lower case).
    words: Option<HashSet<String>>,
    ignored: HashSet<String>,
}

fn user() -> MutexGuard<'static, User> {
    static USER: OnceLock<Mutex<User>> = OnceLock::new();
    USER.get_or_init(|| Mutex::new(User::default())).lock().unwrap_or_else(|e| e.into_inner())
}

/// The default user dictionary: `custom.cus` in the settings folder (`CADADDY_CONFIG_DIR`, else
/// the platform's), or none in the browser.
fn default_custom() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::path::PathBuf;
        let get = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let root = match std::env::var_os("CADADDY_CONFIG_DIR") {
            Some(r) if !r.is_empty() => Some(PathBuf::from(r)),
            _ => match std::env::consts::OS {
                "macos" => get("HOME").map(|h| h.join("Library/Application Support/CADaddy")),
                "windows" => get("APPDATA").map(|r| r.join("CADaddy")),
                _ => get("XDG_CONFIG_HOME").map(|r| r.join("cadaddy")).or_else(|| get("HOME").map(|h| h.join(".config/cadaddy"))),
            },
        };
        root.map(|r| r.join("custom.cus").to_string_lossy().into_owned()).unwrap_or_default()
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}

/// DCTCUST: the user dictionary file.
pub fn dctcust() -> String {
    user().path.clone().unwrap_or_else(default_custom)
}

/// Set DCTCUST; the new file's words are read on next use.
pub fn set_dctcust(path: &str) {
    let mut u = user();
    u.path = Some(path.trim().to_string());
    u.words = None;
}

fn read_custom(path: &str) -> HashSet<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if path.is_empty() || std::fs::metadata(path).map_or(true, |m| m.len() > MAX_CUSTOM_BYTES) {
            return HashSet::new();
        }
        crate::readfile::read_string(path)
            .map(|t| t.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect())
            .unwrap_or_default()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        HashSet::new()
    }
}

/// Run `f` with the user dictionary's words (read on first use) and the ignored words.
fn with_user<R>(f: impl FnOnce(&HashSet<String>, &HashSet<String>) -> R) -> R {
    let mut u = user();
    if u.words.is_none() {
        let path = u.path.clone().unwrap_or_else(default_custom);
        u.words = Some(read_custom(&path));
    }
    let empty = HashSet::new();
    f(u.words.as_ref().unwrap_or(&empty), &u.ignored)
}

/// Whether `word` is spelled right (or added, or ignored).
pub fn is_known(word: &str) -> bool {
    let w = word.replace('\u{2019}', "'");
    let lower = w.to_lowercase();
    let main = main_words();
    let title: String = {
        let mut c = lower.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    };
    if main.contains(w.as_str()) || main.contains(lower.as_str()) || main.contains(title.as_str()) {
        return true;
    }
    // A possessive of a known word.
    if let Some(stem) = lower.strip_suffix("'s").or_else(|| lower.strip_suffix('\'')).filter(|s| !s.is_empty())
        && (main.contains(stem) || is_known_stem(stem, &title))
    {
        return true;
    }
    with_user(|custom, ignored| custom.contains(&lower) || ignored.contains(&lower))
}

fn is_known_stem(stem: &str, title: &str) -> bool {
    let t = title.trim_end_matches("'s").trim_end_matches('\'');
    main_words().contains(t) || with_user(|c, _| c.contains(stem))
}

/// Damerau–Levenshtein distance between `a` and `b`, or `None` once it passes `max`.
fn distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let w = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * w];
    for (i, cell) in d.iter_mut().step_by(w).enumerate() {
        *cell = i;
    }
    for (j, cell) in d.iter_mut().take(w).enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        let mut row_min = usize::MAX;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[(i - 1) * w + j] + 1).min(d[i * w + j - 1] + 1).min(d[(i - 1) * w + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * w + j - 2] + 1);
            }
            d[i * w + j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return None;
        }
    }
    let r = d[a.len() * w + b.len()];
    (r <= max).then_some(r)
}

/// Words close to `word`, nearest first, in the word's own case style (UPPER, Title, lower).
pub fn suggestions(word: &str) -> Vec<String> {
    let lower: Vec<char> = word.to_lowercase().chars().collect();
    if lower.is_empty() || lower.len() > MAX_WORD {
        return Vec::new();
    }
    let max = if lower.len() <= 4 { 1 } else { 2 };
    // The letters with doubled ones once: "wirring" and "wiring" agree there, a usual slip.
    let skeleton = |w: &[char]| -> Vec<char> {
        let mut v = w.to_vec();
        v.dedup();
        v
    };
    let own = skeleton(&lower);
    let mut found: Vec<(usize, bool, &str)> = Vec::new();
    for cand in main_words() {
        let c: Vec<char> = cand.to_lowercase().chars().collect();
        if let Some(d) = distance(&lower, &c, max) {
            found.push((d, skeleton(&c) != own, cand));
        }
    }
    found.sort_by(|a, b| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.len().abs_diff(lower.len()).cmp(&b.2.len().abs_diff(lower.len()))).then(a.2.cmp(b.2))
    });
    let upper = word.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase) && word.chars().filter(|c| c.is_alphabetic()).count() > 1;
    let title = word.chars().next().is_some_and(char::is_uppercase);
    let mut out: Vec<String> = Vec::new();
    for (_, _, w) in found {
        let s = if upper {
            w.to_uppercase()
        } else if title {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        } else {
            w.to_string()
        };
        if !out.contains(&s) {
            out.push(s);
        }
        if out.len() >= MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

/// For each character of `chars`, whether it is shown text (not a format, `%%` or field code).
fn literal_mask(chars: &[char], mtext: bool) -> Vec<bool> {
    let mut mask = vec![true; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let code = if let Some(n) = cadaddy_fonts::field_code(chars, i) {
            n
        } else if let Some((_, n)) = cadaddy_fonts::unicode_escape(chars, i) {
            n
        } else if chars.get(i) == Some(&'%') && chars.get(i + 1) == Some(&'%') {
            match chars.get(i + 2) {
                Some(c) if c.is_ascii_digit() => 2 + chars[i + 2..].iter().take(3).take_while(|c| c.is_ascii_digit()).count(),
                Some(_) => 3,
                None => 2,
            }
        } else if mtext && matches!(chars.get(i), Some('{' | '}')) {
            1
        } else if mtext && chars.get(i) == Some(&'\\') {
            match chars.get(i + 1) {
                None => 1,
                Some('\\' | '{' | '}' | 'P' | 'N' | 'L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'X' | '~') => 2,
                Some(_) => chars[i..].iter().position(|c| *c == ';').map_or(chars.len() - i, |p| p + 1),
            }
        } else if chars.get(i) == Some(&'<') && chars.get(i + 1) == Some(&'>') {
            // The measurement in a dimension text override.
            2
        } else {
            0
        };
        if code == 0 {
            i += 1;
            continue;
        }
        let end = (i + code).min(chars.len());
        mask[i..end].iter_mut().for_each(|m| *m = false);
        i = end;
    }
    mask
}

/// The words of `text` to check: (byte range, word). Runs of letters (with apostrophes inside);
/// runs touching digits are left out.
fn words(text: &str, mtext: bool) -> Vec<(std::ops::Range<usize>, String)> {
    let idx: Vec<(usize, char)> = text.char_indices().collect();
    let chars: Vec<char> = idx.iter().map(|(_, c)| *c).collect();
    let mask = literal_mask(&chars, mtext);
    let is_part = |k: usize| {
        mask[k]
            && (chars[k].is_alphanumeric()
                || (matches!(chars[k], '\'' | '\u{2019}')
                    && k > 0
                    && k + 1 < chars.len()
                    && chars[k - 1].is_alphabetic()
                    && chars[k + 1].is_alphabetic()))
    };
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        if !is_part(k) {
            k += 1;
            continue;
        }
        let start = k;
        while k < chars.len() && is_part(k) {
            k += 1;
        }
        let w: String = chars[start..k].iter().collect();
        let has_digit = w.chars().any(|c| c.is_numeric());
        if !has_digit && w.chars().count() <= MAX_WORD {
            let from = idx[start].0;
            let to = idx.get(k).map_or(text.len(), |(b, _)| *b);
            out.push((from..to, w));
        }
    }
    out
}

/// Where a piece of text is in an object.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Text,
    Attrib(usize),
    DimText,
    Cell(usize, usize),
    MLeader,
}

impl Slot {
    fn json(&self) -> Value {
        match self {
            Slot::Text => json!("text"),
            Slot::Attrib(i) => json!({"attribute": i}),
            Slot::DimText => json!("dimension"),
            Slot::Cell(r, c) => json!({"cell": [r, c]}),
            Slot::MLeader => json!("multileader"),
        }
    }
    fn parse(v: &Value) -> Option<Slot> {
        match v {
            Value::String(s) if s == "text" => Some(Slot::Text),
            Value::String(s) if s == "dimension" => Some(Slot::DimText),
            Value::String(s) if s == "multileader" => Some(Slot::MLeader),
            Value::Object(_) => {
                if let Some(i) = v.get("attribute").and_then(Value::as_u64) {
                    return usize::try_from(i).ok().map(Slot::Attrib);
                }
                let c = v.get("cell")?.as_array()?;
                Some(Slot::Cell(usize::try_from(c.first()?.as_u64()?).ok()?, usize::try_from(c.get(1)?.as_u64()?).ok()?))
            }
            _ => None,
        }
    }
}

/// The texts of `k` to check: where each is, the text, and whether it is MTEXT-coded.
fn texts(k: &EntityKind) -> Vec<(Slot, String, bool)> {
    match k {
        EntityKind::Text(t) => vec![(Slot::Text, t.value.clone(), false)],
        EntityKind::AttDef(a) => vec![(Slot::Text, a.text.value.clone(), false)],
        EntityKind::MText(t) => vec![(Slot::Text, t.contents.clone(), true)],
        EntityKind::Insert(i) => {
            i.attribs.iter().enumerate().filter(|(_, a)| !a.invisible).map(|(n, a)| (Slot::Attrib(n), a.text.value.clone(), false)).collect()
        }
        EntityKind::Dimension(d) if !d.text.is_empty() => vec![(Slot::DimText, d.text.clone(), true)],
        EntityKind::Table(t) => t
            .cells
            .iter()
            .enumerate()
            .flat_map(|(r, row)| {
                row.iter().enumerate().filter(|(_, c)| c.formula.is_none()).map(move |(c, cell)| (Slot::Cell(r, c), cell.text.clone(), true))
            })
            .collect(),
        EntityKind::MLeader(m) => m.text.as_ref().map(|t| vec![(Slot::MLeader, t.contents.clone(), true)]).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Write `text` into `slot` of `k`.
fn put(k: &mut EntityKind, slot: &Slot, text: String) -> bool {
    match (k, slot) {
        (EntityKind::Text(t), Slot::Text) => t.value = text,
        (EntityKind::AttDef(a), Slot::Text) => a.text.value = text,
        (EntityKind::MText(t), Slot::Text) => t.contents = text,
        (EntityKind::Insert(i), Slot::Attrib(n)) => match i.attribs.get_mut(*n) {
            Some(a) => a.text.value = text,
            None => return false,
        },
        (EntityKind::Dimension(d), Slot::DimText) => {
            d.text = text;
            d.block = None;
        }
        (EntityKind::Table(t), Slot::Cell(r, c)) => match t.cells.get_mut(*r).and_then(|row| row.get_mut(*c)) {
            Some(cell) => cell.text = text,
            None => return false,
        },
        (EntityKind::MLeader(m), Slot::MLeader) => match &mut m.text {
            Some(t) => t.contents = text,
            None => return false,
        },
        _ => return false,
    }
    true
}

/// A misspelled word found: the object, where in it, the word and the text around it.
struct Miss {
    handle: Handle,
    slot: Slot,
    word: String,
    context: String,
}

/// The objects to check: `handles`, else every object of the current space.
fn objects(s: &Session, p: &Value) -> Result<Vec<Handle>> {
    if p.get("handles").is_some() || p.get("handle").is_some() {
        return targets(s, p);
    }
    let d = s.doc()?;
    let space = s.space();
    Ok(d.space(&space).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default())
}

fn check(d: &Drawing, hs: &[Handle]) -> Vec<Miss> {
    let mut out = Vec::new();
    for h in hs {
        let Some(e) = d.entity(*h) else { continue };
        for (slot, text, mtext) in texts(&e.kind) {
            let plain = cadaddy_fonts::plain_text(&text);
            for (_, w) in words(&text, mtext) {
                if out.len() >= MAX_FOUND {
                    return out;
                }
                if !is_known(&w) {
                    out.push(Miss { handle: *h, slot: slot.clone(), word: w, context: plain.chars().take(120).collect() });
                }
            }
        }
    }
    out
}

/// Replace whole-word `from` with `to` in `text` (the first occurrence, or all).
fn replace_word(text: &str, mtext: bool, from: &str, to: &str, all: bool) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut done = false;
    for (r, w) in words(text, mtext) {
        if w != from || (done && !all) {
            continue;
        }
        out.push_str(&text[last..r.start]);
        out.push_str(to);
        last = r.end;
        done = true;
    }
    if !done {
        return None;
    }
    out.push_str(&text[last..]);
    Some(out)
}

fn misses_json(m: &[Miss], with_suggestions: bool) -> Value {
    json!(
        m.iter()
            .map(|m| {
                let mut v = json!({"handle": m.handle.hex(), "where": m.slot.json(), "word": m.word, "context": m.context});
                if with_suggestions {
                    v["suggestions"] = json!(suggestions(&m.word));
                }
                v
            })
            .collect::<Vec<_>>()
    )
}

/// Add `word` to the user dictionary (and its file, when there is one).
pub fn add_word(word: &str) -> Result<()> {
    let w = word.trim();
    if w.is_empty() || w.chars().count() > MAX_WORD || w.chars().any(char::is_whitespace) {
        return Err(EngineError::Other("A dictionary word has no spaces and at most 64 letters.".into()));
    }
    let path = dctcust();
    with_user(|_, _| ());
    let mut u = user();
    if let Some(words) = &mut u.words {
        words.insert(w.to_lowercase());
    }
    drop(u);
    #[cfg(not(target_arch = "wasm32"))]
    if !path.is_empty() {
        use std::io::Write;
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        crate::writefile::check(std::path::Path::new(&path)).map_err(|e| EngineError::Other(e.to_string()))?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        writeln!(f, "{w}").map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = path;
    Ok(())
}

/// Change `word` to `to` in object `h` (one occurrence in `slot`, or with `all` every occurrence
/// in every object of `hs`). Returns the objects changed.
fn change(s: &mut Session, hs: &[Handle], h: Option<(Handle, Slot)>, word: &str, to: &str, all: bool) -> Result<Vec<Handle>> {
    if to.chars().count() > 10_000 {
        return Err(EngineError::Other("The replacement is too long.".into()));
    }
    let scope: Vec<Handle> = match (&h, all) {
        (_, true) => hs.to_vec(),
        (Some((h, _)), false) => vec![*h],
        (None, false) => return Err(EngineError::Other("Name the object to change (`handle`), or change all.".into())),
    };
    let mut changed = Vec::new();
    for oh in scope {
        if super::curves::is_locked(s, oh) {
            continue;
        }
        let Some(e) = s.doc()?.entity(oh) else { continue };
        let mut k = e.kind.clone();
        let mut any = false;
        for (slot, text, mtext) in texts(&e.kind) {
            if let Some((_, only)) = &h
                && !all
                && *only != slot
            {
                continue;
            }
            if let Some(t) = replace_word(&text, mtext, word, to, all) {
                any |= put(&mut k, &slot, t);
                if !all {
                    break;
                }
            }
        }
        if any {
            s.doc_mut()?.modify_entity(oh, |e| e.kind = k)?;
            changed.push(oh);
        }
    }
    Ok(changed)
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let action = p.get("action").and_then(Value::as_str).unwrap_or("check").to_ascii_lowercase();
    let word =
        || p.get("word").and_then(Value::as_str).map(str::to_string).ok_or_else(|| bad("spell", format!("`word` is required for \"{action}\"")));
    match action.as_str() {
        "check" => {
            let hs = objects(s, p)?;
            let found = check(s.doc()?, &hs);
            Ok(json!({ "misspelled": misses_json(&found, bool_or(p, "suggestions", true)), "checked": hs.len() }))
        }
        "suggest" => Ok(json!({ "suggestions": suggestions(&word()?) })),
        "add" => {
            add_word(&word()?)?;
            Ok(json!({ "added": word()?, "dictionary": dctcust() }))
        }
        "ignoreall" | "ignore_all" => {
            user().ignored.insert(word()?.to_lowercase());
            Ok(json!({ "ignored": word()? }))
        }
        "change" | "changeall" => {
            let all = action == "changeall" || bool_or(p, "all", false);
            let to = p.get("to").and_then(Value::as_str).ok_or_else(|| bad("spell", "`to` (the new word) is required"))?.to_string();
            let target = match super::curves::handle_param(p, "handle") {
                Some(h) => Some((
                    h,
                    p.get("where")
                        .map_or(Some(Slot::Text), Slot::parse)
                        .ok_or_else(|| bad("spell", "`where` is \"text\", \"dimension\", \"multileader\", {attribute: n} or {cell: [row, col]}"))?,
                )),
                None => None,
            };
            let hs = if all { objects(s, &json!({}))? } else { Vec::new() };
            let changed = change(s, &hs, target, &word()?, &to, all)?;
            if changed.is_empty() {
                return Err(EngineError::Other(format!("\"{}\" was not found.", word()?)));
            }
            Ok(json!({ "changed": changed.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
        }
        other => Err(bad("spell", format!("unknown action \"{other}\" (check, suggest, change, changeAll, ignoreAll or add)"))),
    }
}

// ---------------- command line ----------------

#[derive(Default)]
pub(crate) struct SpellM {
    sel: SelectPhase,
    objs: Vec<Handle>,
    queue: Vec<(Handle, Slot, String)>,
    /// Words passed over with Ignore (object and word), left out when checking again.
    skipped: HashSet<(Handle, String)>,
    changing: bool,
    all: bool,
}

const KEYS: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

impl SpellM {
    /// Check again from the current objects, skipping words ignored this run.
    fn refill(&mut self, s: &Session) -> Result<()> {
        let d = s.doc()?;
        self.queue = check(d, &self.objs)
            .into_iter()
            .filter(|m| !self.skipped.contains(&(m.handle, m.word.clone())))
            .map(|m| (m.handle, m.slot, m.word))
            .collect();
        self.queue.reverse();
        Ok(())
    }
    fn next(&mut self, s: &mut Session) -> Result<Step> {
        if self.queue.is_empty() {
            s.echo("Spelling check complete.");
            return Ok(Step::Done);
        }
        Ok(Step::Continue)
    }
}

impl Interactive for SpellM {
    fn name(&self) -> &'static str {
        "SPELL"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
            self.refill(s)?;
            return self.next(s);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.sel.done {
            return self.sel.prompt();
        }
        let Some((_, _, w)) = self.queue.last() else { return Prompt::new("Spelling check complete", KEYS) };
        if self.changing {
            let first = suggestions(w).into_iter().next().unwrap_or_else(|| w.clone());
            return Prompt::new(format!("Change \"{w}\" to"), Accept::TEXT).default(first);
        }
        let sug = suggestions(w);
        let msg = if sug.is_empty() {
            format!("\"{w}\" is not in the dictionary")
        } else {
            format!("\"{w}\" is not in the dictionary (suggestions: {})", sug.join(", "))
        };
        Prompt::new(msg, KEYS).kw(&["Ignore", "ignore All", "Change", "change aLl", "Add", "eXit"]).default("Ignore")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Cancel),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    self.refill(s)?;
                    self.next(s)
                }
            };
        }
        let Some((h, slot, w)) = self.queue.last().cloned() else { return Ok(Step::Done) };
        if self.changing {
            let to = match &i {
                Input::Text(t) if !t.trim().is_empty() => t.trim().to_string(),
                Input::Text(_) | Input::Enter => suggestions(&w).into_iter().next().unwrap_or_else(|| w.clone()),
                _ => return Ok(Step::Continue),
            };
            let all = self.all;
            change(s, &self.objs.clone(), Some((h, slot)), &w, &to, all)?;
            self.changing = false;
            self.refill(s)?;
            return self.next(s);
        }
        match &i {
            Input::Enter => {
                self.skipped.insert((h, w.clone()));
                self.queue.pop();
            }
            Input::Keyword(k) => match k.as_str() {
                "Ignore" => {
                    self.skipped.insert((h, w.clone()));
                    self.queue.pop();
                }
                "ignore All" => {
                    user().ignored.insert(w.to_lowercase());
                    self.queue.retain(|(_, _, x)| !x.eq_ignore_ascii_case(&w));
                }
                "Change" => {
                    self.changing = true;
                    self.all = false;
                    return Ok(Step::Continue);
                }
                "change aLl" => {
                    self.changing = true;
                    self.all = true;
                    return Ok(Step::Continue);
                }
                "Add" => {
                    add_word(&w)?;
                    self.queue.retain(|(_, _, x)| !x.eq_ignore_ascii_case(&w));
                }
                _ => return Ok(Step::Done),
            },
            _ => return Err(EngineError::Other("Invalid option keyword.".into())),
        }
        self.next(s)
    }
}
