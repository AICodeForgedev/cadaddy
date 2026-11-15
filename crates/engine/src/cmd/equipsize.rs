//! Equipment boxes sized by their approved submittals (`bim.equipment_sizes`, Q6655).
//!
//! A schedule often prints no size, and the unit bought is often another maker's than the
//! schedule's (a substitution). The field sets the unit the approved submittal shows: its cut
//! sheet's height, width and depth, on the curb or rails it gives. So a piece of equipment takes
//! its box from its approved submittal first, else from its schedule, else its kind's usual size
//! (marked as such). A roof unit's curb height comes from the same submittal. A size the schedule
//! or the plan gave stays beside the submittal's as the design value (`corrections`), with the
//! page and the rule (`M-EQUIP-SUBMITTAL-SIZE`). A submittal sent back (revise and resubmit,
//! rejected) is not built from.
//!
//! A unit's pages are the pages naming its tag (`HP-9`, `HP 9`; `QC/QCP-4` names QC-4 and QCP-4)
//! or its schedule's model number. The pages after such a page that name no unit belong to it
//! too: a unit's cover sheet comes first, then its data sheets. Its size is the labelled height,
//! width and depth on those pages, in any of these forms:
//! - `Height 31.89 Width 37.24 Depth 16.14`
//! - `31.89" H x 37.24" W x 16.14" D`
//! - `Dimensions (W x D x H): 37.24 x 16.14 x 31.89`
//! - a header row over its values.
//!
//! An accessory's size (a controller, a wireless module) is not the unit's, and neither is a
//! shipping size, an opening or a piping length (the rules data's words).
//!
//! The job sizes the units this way while it builds them. On a built model,
//! `bim.equipment_sizes` resizes the units in place: same elements, same properties. Rebuilding
//! the model is not needed to resize one unit.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::{BuildingSpec, MepEquipment, RunCorrection};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The rule a submittal-sized box carries (rules data: `tradepractice.json`).
pub(crate) const RULE: &str = "M-EQUIP-SUBMITTAL-SIZE";
const CMD: &str = "bim.equipment_sizes";
/// Submittal files read in one pass, pages read of each, the largest file read.
const MAX_FILES: usize = 120;
const MAX_PAGES: u32 = 80;
const MAX_FILE_BYTES: usize = 80 * 1024 * 1024;
/// Submittal files looked at, and pages read of one to tell whether it is about a unit.
const MAX_LISTED_FILES: usize = 2_000;
const PROBE_PAGES: u32 = 2;
/// A page naming more units than this is a list (a schedule, an index), not one unit's sheet.
const MAX_PAGE_TAGS: usize = 8;
/// Lines of one page and words of one line looked at.
const MAX_LINES: usize = 4_000;
const MAX_TOKENS: usize = 400;
/// A unit's dimension, inches: anything outside is not one.
const MIN_IN: f64 = 0.5;
const MAX_IN: f64 = 1_200.0;
/// A curb's or rail's height, inches.
const CURB_MIN_IN: f64 = 4.0;
const CURB_MAX_IN: f64 = 72.0;
/// Lines a header row's values may be below it; lines between the parts of one size.
const HEADER_REACH: usize = 3;
const SET_REACH: usize = 3;
/// Lines at a page's top read as its heading (what its sizes are of).
const HEADING_LINES: usize = 4;
/// Lines above a size read with it (what it is the size of).
const CONTEXT_LINES: usize = 2;
/// Units looked at, and resized units listed in a report.
const MAX_UNITS: usize = 20_000;
const MAX_LISTED: usize = 200;
/// A size within this of the box's (inches) is the box's.
const SAME_IN: f64 = 0.25;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Equipment Sizes from Submittals", run)
            .params("{paths?: [path] (submittal PDFs or folders; default: the job's submittals), dry_run?: bool} → {files_read, sent_back, unread, units, sized: {submittal, schedule, drawn, rule, other} (before and after), resized: [{tag, was, now, source, curb_in?}], confirmed, curbs, elements_updated}: each piece of equipment takes the height, width and depth its approved submittal gives (a substitution's too), and a roof unit its curb's height; the schedule's or the plan's size is kept beside it as the design value (M-EQUIP-SUBMITTAL-SIZE); built elements are resized in place")
    ]
}

/// One axis of a size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Axis {
    H,
    W,
    D,
    L,
}

/// A length's unit as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Len {
    In,
    Mm,
    Cm,
    Ft,
}

impl Len {
    fn inches(self, v: f64) -> f64 {
        match self {
            Len::In => v,
            Len::Mm => v / 25.4,
            Len::Cm => v / 2.54,
            Len::Ft => v * 12.0,
        }
    }
}

/// A word of a line, as the size reader sees it.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// A number, with its unit when written on it.
    Num(f64, Option<Len>),
    /// A dimension label: `HEIGHT` (a word: `true`) or `H` (a letter: `false`).
    Label(Axis, bool),
    /// The `x` between the parts of a size.
    X,
    Unit(Len),
    /// A number in brackets after another (the metric echo: `31.89 (810)`).
    Echo,
    Word(String),
}

/// The rules data's word lists ([`RULE`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Vocab {
    not_unit: BTreeSet<String>,
    accessory: Vec<String>,
    generic: BTreeSet<String>,
    /// Per equipment kind, the words a submittal's file name may call it by (`heat_pump`:
    /// `SPLIT`, `DUCTLESS`).
    kind_words: BTreeMap<String, Vec<String>>,
    /// A split system's sections: the kinds of each side and the words that name it.
    outdoor_kinds: BTreeSet<String>,
    indoor_kinds: BTreeSet<String>,
    outdoor_words: Vec<String>,
    indoor_words: Vec<String>,
    /// Words a number after them doesn't make a tag of (`HP 1` is a horsepower, `PAGE 2` a page).
    pub(crate) not_tag: BTreeSet<String>,
}

impl Vocab {
    pub(crate) fn load() -> Self {
        let data = super::provenance::rule_data(RULE);
        let list = |k: &str| -> Vec<String> {
            data.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|w| w.trim().to_ascii_uppercase())
                .filter(|w| !w.is_empty())
                .take(500)
                .collect()
        };
        let kind_words = data
            .get("kind_words")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .take(200)
            .map(|(k, v)| {
                let words = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|w| w.trim().to_ascii_uppercase())
                    .filter(|w| !w.is_empty())
                    .take(50);
                (k.to_ascii_lowercase(), words.collect())
            })
            .collect();
        Vocab {
            not_unit: list("not_unit_words").into_iter().collect(),
            accessory: list("accessory_words"),
            generic: list("generic_words").into_iter().collect(),
            kind_words,
            outdoor_kinds: list("outdoor_kinds").into_iter().map(|k| k.to_ascii_lowercase()).collect(),
            indoor_kinds: list("indoor_kinds").into_iter().map(|k| k.to_ascii_lowercase()).collect(),
            outdoor_words: list("outdoor_words"),
            indoor_words: list("indoor_words"),
            not_tag: list("not_tag_words").into_iter().collect(),
        }
    }
}

/// One size read off a page: the parts it gives, inches, and the text around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dims {
    h: Option<f64>,
    w: Option<f64>,
    d: Option<f64>,
    l: Option<f64>,
    /// Its lines and the ones just above, upper case.
    context: String,
    /// Its first line and the ones above it, nearest first (upper case).
    lead: Vec<String>,
}

impl Dims {
    fn get(&self, a: Axis) -> Option<f64> {
        match a {
            Axis::H => self.h,
            Axis::W => self.w,
            Axis::D => self.d,
            Axis::L => self.l,
        }
    }
    fn set(&mut self, a: Axis, v: f64) {
        match a {
            Axis::H => self.h = Some(v),
            Axis::W => self.w = Some(v),
            Axis::D => self.d = Some(v),
            Axis::L => self.l = Some(v),
        }
    }
    /// Height, width (the plan's long side), depth: a height and two plan sides. A length and a
    /// width with no depth are the long and the short side.
    pub(crate) fn size_in(&self) -> Option<[f64; 3]> {
        let h = self.h?;
        let (w, d) = match (self.w, self.d, self.l) {
            (Some(w), Some(d), Some(l)) => (w.max(l), d),
            (Some(w), Some(d), None) => (w, d),
            (None, Some(d), Some(l)) => (l, d),
            (Some(w), None, Some(l)) => (w.max(l), w.min(l)),
            _ => return None,
        };
        let ok = |v: f64| v.is_finite() && (MIN_IN..=MAX_IN).contains(&v);
        (ok(h) && ok(w) && ok(d)).then_some([h, w, d])
    }
    fn volume(&self) -> f64 {
        self.size_in().map_or(0.0, |[h, w, d]| h * w * d)
    }
}

/// A word's dimension label: `HEIGHT`, `HT` … (a word), `H` (a letter).
fn label(t: &str) -> Option<(Axis, bool)> {
    let t = t.trim_end_matches([':', '=', '.', ',']);
    let word = match t {
        "HEIGHT" | "HT" | "HGT" | "HEIGHTS" => Some(Axis::H),
        "WIDTH" | "WD" | "WIDTHS" => Some(Axis::W),
        "DEPTH" | "DP" | "DPTH" | "DEPTHS" => Some(Axis::D),
        "LENGTH" | "LG" | "LEN" | "LGTH" | "LENGTHS" => Some(Axis::L),
        _ => None,
    };
    if let Some(a) = word {
        return Some((a, true));
    }
    match t {
        "H" => Some((Axis::H, false)),
        "W" => Some((Axis::W, false)),
        "D" => Some((Axis::D, false)),
        "L" => Some((Axis::L, false)),
        _ => None,
    }
}

/// A unit word on its own: `IN`, `(IN.)`, `INCHES`, `MM`, `"`.
fn unit_word(t: &str) -> Option<Len> {
    let t = t.trim_matches(['(', ')', '[', ']', ',', ':']);
    match t {
        "IN" | "IN." | "INS" | "INCH" | "INCHES" | "\"" | "''" | "\u{201D}" | "\u{2033}" | "IN.(MM)" | "IN(MM)" | "IN./MM" | "IN/MM" => Some(Len::In),
        "MM" | "MM." | "MILLIMETERS" | "MILLIMETRES" => Some(Len::Mm),
        "CM" | "CM." => Some(Len::Cm),
        "FT" | "FT." | "FEET" | "'" => Some(Len::Ft),
        _ => None,
    }
}

/// A plain or mixed number: `31.89`, `1,325`, `11/16`, `19-11/16`.
fn plain_number(t: &str) -> Option<f64> {
    let t = t.trim();
    if !t.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let frac = |s: &str| -> Option<f64> {
        let (a, b) = s.split_once('/')?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        (b > 0.0 && a.is_finite() && b.is_finite()).then(|| a / b)
    };
    let v = if t.contains('/') {
        match t.split_once('-') {
            Some((whole, f)) => whole.trim().parse::<f64>().ok()? + frac(f)?,
            None => frac(t)?,
        }
    } else {
        if !t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
            return None;
        }
        t.replace(',', "").parse::<f64>().ok()?
    };
    (v.is_finite() && v > 0.0).then_some(v)
}

/// A length as one word: `31.89`, `31.89"`, `31.89IN`, `810MM`, `2'-6"`, `19-11/16"`,
/// `31.89(810)` (the bracketed echo dropped).
fn length(t: &str) -> Option<(f64, Option<Len>)> {
    let t = t.trim_end_matches([',', ';', ':']);
    // The metric echo in brackets right after the number.
    let t = match t.find('(') {
        Some(i) if i > 0 => t.get(..i).unwrap_or(t),
        _ => t,
    };
    if !t.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    // Feet and inches.
    if let Some((ft, rest)) = t.split_once('\'') {
        let rest = rest.trim_start_matches('-').trim_end_matches(['"', '\u{201D}', '\u{2033}']).trim_end_matches("''");
        let ft = plain_number(ft)?;
        let inch = if rest.is_empty() { 0.0 } else { plain_number(rest)? };
        return Some((ft * 12.0 + inch, Some(Len::In)));
    }
    for (suffix, unit) in [
        ("MM", Len::Mm),
        ("CM", Len::Cm),
        ("IN.", Len::In),
        ("IN", Len::In),
        ("\"", Len::In),
        ("\u{201D}", Len::In),
        ("\u{2033}", Len::In),
        ("FT.", Len::Ft),
        ("FT", Len::Ft),
    ] {
        if let Some(n) = t.strip_suffix(suffix) {
            return plain_number(n).map(|v| (v, Some(unit)));
        }
    }
    plain_number(t).map(|v| (v, None))
}

/// One raw word as tokens: `37.24X16.14X31.89` and `(WXDXH)` split at their `x`.
fn word_tokens(raw: &str, out: &mut Vec<Tok>) {
    let w = raw.trim_matches([',', ';']);
    if w.is_empty() {
        return;
    }
    if w == "X" || w == "×" || w == "*" {
        out.push(Tok::X);
        return;
    }
    // A bracketed number on its own: a metric echo.
    if w.starts_with('(') && w.ends_with(')') && w.len() > 2 && plain_number(w.trim_matches(['(', ')'])).is_some() {
        out.push(Tok::Echo);
        return;
    }
    if let Some(u) = unit_word(w) {
        out.push(Tok::Unit(u));
        return;
    }
    // Without the brackets and punctuation round it (`(W`, `H):`, `WIDTH,`).
    let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | ':' | '='));
    if w.is_empty() {
        return;
    }
    // A label with its unit on it: `HEIGHT(IN)`, `DEPTH(MM)`.
    let (core, unit) = match w.split_once('(') {
        Some((a, b)) if label(a).is_some() => (a, unit_word(b)),
        _ => (w, None),
    };
    if let Some((a, word)) = label(core) {
        out.push(Tok::Label(a, word));
        if let Some(u) = unit {
            out.push(Tok::Unit(u));
        }
        return;
    }
    if let Some((v, u)) = length(w) {
        out.push(Tok::Num(v, u));
        return;
    }
    // A length with its letter on it: `16"D`, `31-7/8"H`.
    if let Some(last) = w.chars().last().filter(|c| matches!(c, 'H' | 'W' | 'D' | 'L'))
        && let Some((v, u)) = w
            .get(..w.len() - 1)
            .filter(|r| r.ends_with(['"', '.', '\u{201D}', '\u{2033}']) || r.ends_with(|c: char| c.is_ascii_digit()))
            .and_then(length)
        && let Some((a, word)) = label(&last.to_string())
    {
        out.push(Tok::Num(v, u));
        out.push(Tok::Label(a, word));
        return;
    }
    // Parts joined by x: every part a length or a label.
    let parts: Vec<&str> = core.split(['X', '×', '*']).collect();
    if parts.len() >= 2 && parts.len() <= 4 {
        let toks: Vec<Option<Tok>> = parts
            .iter()
            .map(|p| {
                let p = p.trim_matches(['(', ')']);
                label(p).map(|(a, word)| Tok::Label(a, word)).or_else(|| length(p).map(|(v, u)| Tok::Num(v, u)))
            })
            .collect();
        if toks.iter().all(Option::is_some) {
            for (i, t) in toks.into_iter().flatten().enumerate() {
                if i > 0 {
                    out.push(Tok::X);
                }
                out.push(t);
            }
            return;
        }
    }
    out.push(Tok::Word(w.to_string()));
}

/// A line's tokens; a whole number followed by a fraction is one number (`19 11/16`).
fn tokens(line: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for raw in line.to_ascii_uppercase().split_whitespace().take(MAX_TOKENS) {
        let before = out.len();
        word_tokens(raw, &mut out);
        // `19 11/16"`: the fraction joins the whole number before it.
        if out.len() == before + 1
            && raw.contains('/')
            && let (Some(Tok::Num(f, fu)), Some(Tok::Num(whole, None))) =
                (out.get(before).cloned(), before.checked_sub(1).and_then(|i| out.get(i)).cloned())
            && whole.fract() == 0.0
            && f < 1.0
        {
            out.truncate(before - 1);
            out.push(Tok::Num(whole + f, fu));
        }
    }
    out
}

/// What the size reader is after: a unit's size, or its curb's (where `SUPPLY HEIGHT` is the
/// curb's height and `CURB` names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Unit,
    Curb,
}

/// The words that make a label a curb's in a curb's line (`30" SUPPLY HEIGHT`).
const CURB_QUALIFIERS: &[&str] = &["CURB", "SUPPLY", "RETURN", "OVERALL", "RAIL", "RAILS"];

/// A line's sizes: (axis, inches) in order, from the `a x b x c` runs (labelled by letters, or by
/// an order the line or the one before gives: `order`) and the word labels' values (after the
/// label, else before it). The order a line gives for the next one comes back as well.
fn bind(toks: &[Tok], order_before: Option<&[Axis]>, mode: Mode, vocab: &Vocab) -> (Vec<(Axis, f64)>, Option<Vec<Axis>>) {
    let mut used = vec![false; toks.len()];
    let mut out: Vec<(Axis, f64)> = Vec::new();
    let unit_after = |i: usize| match toks.get(i + 1) {
        Some(Tok::Unit(u)) => Some(*u),
        _ => None,
    };
    // An order of labels joined by x: `(W x D x H)`, `HEIGHT X WIDTH X DEPTH`.
    let mut order: Option<Vec<Axis>> = None;
    let mut i = 0;
    while i < toks.len() {
        if let Some(Tok::Label(a, _)) = toks.get(i) {
            let mut axes = vec![*a];
            let mut j = i;
            while let (Some(Tok::X), Some(Tok::Label(b, _))) = (toks.get(j + 1), toks.get(j + 2)) {
                axes.push(*b);
                j += 2;
            }
            let distinct = axes.iter().collect::<BTreeSet<_>>().len() == axes.len();
            if axes.len() >= 2 && distinct {
                for k in i..=j {
                    if let Some(u) = used.get_mut(k) {
                        *u = true;
                    }
                }
                order = Some(axes);
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    // The runs of lengths joined by x.
    let mut i = 0;
    while i < toks.len() {
        if used.get(i).copied().unwrap_or(true) || !matches!(toks.get(i), Some(Tok::Num(..))) {
            i += 1;
            continue;
        }
        // (value inches?, its letter or word label) per part.
        let mut parts: Vec<(f64, Option<Len>, Option<Axis>)> = Vec::new();
        let mut j = i;
        while let Some(Tok::Num(v, u)) = toks.get(j) {
            let mut k = j + 1;
            let mut unit = *u;
            if let Some(Tok::Unit(u2)) = toks.get(k) {
                unit = unit.or(Some(*u2));
                k += 1;
            }
            if matches!(toks.get(k), Some(Tok::Echo)) {
                k += 1;
            }
            let lab = match toks.get(k) {
                Some(Tok::Label(a, _)) => {
                    k += 1;
                    Some(*a)
                }
                _ => None,
            };
            parts.push((*v, unit, lab));
            if matches!(toks.get(k), Some(Tok::X)) && matches!(toks.get(k + 1), Some(Tok::Num(..))) {
                j = k + 1;
            } else {
                j = k;
                break;
            }
        }
        if parts.len() >= 2 {
            let line_unit = parts.iter().find_map(|p| p.1).unwrap_or(Len::In);
            let labelled = parts.iter().all(|p| p.2.is_some());
            let ord = order.as_deref().or(order_before);
            let other = toks.iter().take(i).any(|t| matches!(t, Tok::Word(w) if vocab.not_unit.contains(w.as_str())));
            let axes: Option<Vec<Axis>> = if labelled {
                Some(parts.iter().filter_map(|p| p.2).collect())
            } else {
                ord.filter(|o| o.len() == parts.len() && !other).map(<[Axis]>::to_vec)
            };
            if let Some(axes) = axes.filter(|a| a.iter().collect::<BTreeSet<_>>().len() == a.len()) {
                for (p, a) in parts.iter().zip(axes) {
                    out.push((a, p.1.unwrap_or(line_unit).inches(p.0)));
                }
            }
            for k in i..j {
                if let Some(u) = used.get_mut(k) {
                    *u = true;
                }
            }
        }
        i = j.max(i + 1);
    }
    // Word labels: a line writes its values after their labels (`HEIGHT: 31.89 IN`) or before
    // them (`49.5" WIDTH, 75" LENGTH`), as its first label shows; each label takes its value that
    // way, else the other.
    let skip_curb = |i: usize| -> bool {
        mode == Mode::Curb && matches!(i.checked_sub(1).and_then(|p| toks.get(p)), Some(Tok::Word(w)) if CURB_QUALIFIERS.contains(&w.as_str()))
    };
    // The value after label `i` (past unit words): (its place, inches).
    let after = |i: usize, used: &[bool]| -> Option<(usize, f64)> {
        let mut k = i + 1;
        let mut unit: Option<Len> = None;
        while let Some(Tok::Unit(u)) = toks.get(k) {
            unit = unit.or(Some(*u));
            k += 1;
        }
        let Some(Tok::Num(v, u)) = toks.get(k) else { return None };
        (!used.get(k).copied().unwrap_or(true)).then(|| (k, u.or(unit).or(unit_after(k)).unwrap_or(Len::In).inches(*v)))
    };
    // The value before label `i` (past unit words, and a curb's qualifier in a curb's line).
    let before = |i: usize, used: &[bool]| -> Option<(usize, f64)> {
        let mut p = i.checked_sub(1);
        if skip_curb(i) {
            p = p.and_then(|p| p.checked_sub(1));
        }
        let mut unit: Option<Len> = None;
        while let Some(q) = p {
            match toks.get(q) {
                Some(Tok::Unit(u)) => {
                    unit = Some(*u);
                    p = q.checked_sub(1);
                }
                _ => break,
            }
        }
        let q = p?;
        let Some(Tok::Num(v, u)) = toks.get(q) else { return None };
        (!used.get(q).copied().unwrap_or(true)).then(|| (q, u.or(unit).unwrap_or(Len::In).inches(*v)))
    };
    let first_label = toks.iter().enumerate().position(|(i, t)| matches!(t, Tok::Label(_, true)) && !used.get(i).copied().unwrap_or(true));
    let label_next = |k: usize| {
        let mut q = k + 1;
        while matches!(toks.get(q), Some(Tok::Unit(_) | Tok::Echo)) {
            q += 1;
        }
        matches!(toks.get(q), Some(Tok::Label(_, true)))
    };
    let values_first = first_label.is_some_and(|i| {
        before(i, &used).is_some()
            && match after(i, &used) {
                None => true,
                Some((k, _)) => label_next(k),
            }
    });
    for (i, t) in toks.iter().enumerate() {
        let Tok::Label(a, true) = t else { continue };
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        // The word before it may make it something else's (`SHIPPING HEIGHT`).
        let qualifier = match i.checked_sub(1).and_then(|p| toks.get(p)) {
            Some(Tok::Word(w)) => Some(w.as_str()),
            _ => None,
        };
        if qualifier.is_some_and(|q| vocab.not_unit.contains(q)) && !skip_curb(i) {
            continue;
        }
        let got = if values_first { before(i, &used).or_else(|| after(i, &used)) } else { after(i, &used).or_else(|| before(i, &used)) };
        if let Some((k, v)) = got {
            out.push((*a, v));
            if let Some(x) = used.get_mut(k) {
                *x = true;
            }
        }
    }
    (out, order)
}

/// A header row's labels in order: two or more labels and no numbers (`WIDTH DEPTH HEIGHT`,
/// `W D H` when that is all the line holds).
fn header(toks: &[Tok]) -> Option<Vec<Axis>> {
    if toks.iter().any(|t| matches!(t, Tok::Num(..))) {
        return None;
    }
    let labels: Vec<(Axis, bool)> = toks.iter().filter_map(|t| if let Tok::Label(a, w) = t { Some((*a, *w)) } else { None }).collect();
    let letters_only = toks.iter().all(|t| matches!(t, Tok::Label(..) | Tok::Unit(_)));
    let axes: Vec<Axis> = labels.iter().filter(|(_, w)| *w || letters_only).map(|(a, _)| *a).collect();
    (axes.len() >= 2 && axes.iter().collect::<BTreeSet<_>>().len() == axes.len()).then_some(axes)
}

/// The sizes a page gives, in order ([`Dims`]); a part repeating, or lines apart, starts another.
pub(crate) fn dims_in(page: &str, vocab: &Vocab) -> Vec<Dims> {
    let lines: Vec<&str> = page.lines().take(MAX_LINES).collect();
    let toks: Vec<Vec<Tok>> = lines.iter().map(|l| tokens(l)).collect();
    let mut sets: Vec<Dims> = Vec::new();
    let mut cur: Option<(Dims, usize, usize)> = None;
    let mut order: Option<Vec<Axis>> = None;
    let mut pending_header: Option<(Vec<Axis>, usize)> = None;
    let lead = |first: usize| -> Vec<String> {
        (0..=CONTEXT_LINES).filter_map(|k| first.checked_sub(k)).filter_map(|i| lines.get(i)).map(|l| l.to_ascii_uppercase()).collect()
    };
    let context = |first: usize, last: usize| -> String {
        let a = first.saturating_sub(CONTEXT_LINES);
        lines.get(a..=last.min(lines.len().saturating_sub(1))).map(|ls| ls.join(" \n ").to_ascii_uppercase()).unwrap_or_default()
    };
    for (n, t) in toks.iter().enumerate() {
        // A curb's line is the curb's, not the unit's.
        if t.iter().any(|x| matches!(x, Tok::Word(w) if w.contains("CURB"))) {
            order = None;
            continue;
        }
        let (mut got, line_order) = bind(t, order.as_deref(), Mode::Unit, vocab);
        // A header row's values on a line just below it.
        if got.is_empty()
            && let Some((axes, at)) = &pending_header
            && n - at <= HEADER_REACH
        {
            let nums: Vec<(f64, Option<Len>)> = t.iter().filter_map(|x| if let Tok::Num(v, u) = x { Some((*v, *u)) } else { None }).collect();
            let unit = t.iter().find_map(|x| if let Tok::Unit(u) = x { Some(*u) } else { None });
            if nums.len() == axes.len() && !t.iter().any(|x| matches!(x, Tok::Label(..))) {
                got = axes.iter().zip(&nums).map(|(a, (v, u))| (*a, u.or(unit).unwrap_or(Len::In).inches(*v))).collect();
                pending_header = None;
            }
        }
        if let Some(h) = header(t) {
            pending_header = Some((h, n));
        }
        order = line_order;
        for (a, v) in got {
            let fresh = match &cur {
                Some((d, _, last)) => d.get(a).is_some() || n.saturating_sub(*last) > SET_REACH,
                None => true,
            };
            if fresh {
                if let Some((mut d, first, last)) = cur.take() {
                    d.context = context(first, last);
                    d.lead = lead(first);
                    sets.push(d);
                }
                cur = Some((Dims::default(), n, n));
            }
            if let Some((d, _, last)) = cur.as_mut() {
                d.set(a, v);
                *last = n;
            }
        }
    }
    if let Some((mut d, first, last)) = cur.take() {
        d.context = context(first, last);
        d.lead = lead(first);
        sets.push(d);
    }
    sets
}

/// What a page says the unit's curb (or rails) is: its height, and its width and length when
/// given (inches).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Curb {
    pub height_in: f64,
    pub plan_in: Option<[f64; 2]>,
}

/// The curb a page gives: a curb's line with its height (`ROOF CURB - 49.5" WIDTH, 75" LENGTH,
/// 30" SUPPLY HEIGHT`, `CURB HEIGHT: 14`, `ON A 14" CURB`). A number with no length unit right
/// before `CURB` (`146 CURB` in a weight line) is not one.
pub(crate) fn curb_in(page: &str, vocab: &Vocab) -> Option<Curb> {
    let ok = |v: f64| v.is_finite() && (CURB_MIN_IN..=CURB_MAX_IN).contains(&v);
    for line in page.lines().take(MAX_LINES) {
        let up = line.to_ascii_uppercase();
        if !up.contains("CURB") {
            continue;
        }
        let toks = tokens(line);
        let (got, _) = bind(&toks, None, Mode::Curb, vocab);
        let part = |a: Axis| got.iter().find(|(b, _)| *b == a).map(|(_, v)| *v);
        let plan = match (part(Axis::W), part(Axis::L).or(part(Axis::D))) {
            (Some(w), Some(l)) if w > 0.0 && l > 0.0 => Some([w.max(l), w.min(l)]),
            _ => None,
        };
        if let Some(h) = part(Axis::H).filter(|h| ok(*h)) {
            return Some(Curb { height_in: h, plan_in: plan });
        }
        // `14" CURB`, `14 IN. HIGH CURB`, `14" TALL ROOF CURB`: a length with its unit.
        for (i, t) in toks.iter().enumerate() {
            let Tok::Num(v, u) = t else { continue };
            let (unit, mut k) = match (u, toks.get(i + 1)) {
                (Some(u), _) => (Some(*u), i + 1),
                (None, Some(Tok::Unit(u2))) => (Some(*u2), i + 2),
                _ => (None, i + 1),
            };
            let Some(unit) = unit.filter(|u| *u == Len::In || *u == Len::Mm || *u == Len::Cm) else { continue };
            if matches!(toks.get(k), Some(Tok::Word(w)) if w == "HIGH" || w == "TALL") || matches!(toks.get(k), Some(Tok::Label(Axis::H, _))) {
                k += 1;
            }
            let curb_next = (k..k + 3).any(|q| matches!(toks.get(q), Some(Tok::Word(w)) if w.contains("CURB")));
            let h = unit.inches(*v);
            if curb_next && ok(h) {
                return Some(Curb { height_in: h, plan_in: plan });
            }
        }
    }
    None
}

/// A tag the way pages are matched by it: `HP-9`, `HP9`, `hp 9` → `HP-9`. None for a mark with no
/// letters before its number.
pub(crate) fn canon(tag: &str) -> Option<String> {
    let t: String = tag.trim().to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
    let prefix: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    if prefix.is_empty() || prefix.len() > 8 {
        return None;
    }
    let rest = t.get(prefix.len()..)?.trim_start_matches(['-', '#', '_', '.']);
    if !rest.starts_with(|c: char| c.is_ascii_digit()) || rest.len() > 12 || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return None;
    }
    Some(format!("{prefix}-{rest}"))
}

/// The tags a page's words could be: each word's (`HP-9`, `HP9`), a letters word with the number
/// after it (`QH 9`; not a measure's: `HP 1` is a horsepower, `not_tag`), and a shared number
/// (`QC/QCP-4` → `QC-4`, `QCP-4`).
fn tags_named(page: &str, not_tag: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let words: Vec<String> = page
        .to_ascii_uppercase()
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | ':' | '[' | ']' | '&'))
        .filter(|w| !w.is_empty())
        .take(200_000)
        .map(|w| w.trim_matches(['.', '"', '\'']).to_string())
        .collect();
    for (i, w) in words.iter().enumerate() {
        if w.contains('/') {
            let parts: Vec<&str> = w.split('/').filter(|p| !p.is_empty()).collect();
            if let Some(last) = parts.last().and_then(|p| canon(p)) {
                out.insert(last.clone());
                let num = last.split_once('-').map(|(_, n)| n.to_string()).unwrap_or_default();
                for p in parts.iter().take(parts.len().saturating_sub(1)) {
                    match canon(p) {
                        Some(c) => {
                            out.insert(c);
                        }
                        None if p.chars().all(|c| c.is_ascii_alphabetic()) => {
                            if let Some(c) = canon(&format!("{p}-{num}")) {
                                out.insert(c);
                            }
                        }
                        None => {}
                    }
                }
            }
            continue;
        }
        if let Some(c) = canon(w) {
            // A list after it shares its letters (`QF-1, 2, 3`).
            let letters = c.split('-').next().unwrap_or_default().to_string();
            for next in words.iter().skip(i + 1).take(12).take_while(|n| n.chars().all(|ch| ch.is_ascii_digit())) {
                if let Some(t) = canon(&format!("{letters}-{next}")) {
                    out.insert(t);
                }
            }
            out.insert(c);
        } else if w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic()) && !not_tag.contains(w.as_str()) {
            // `HP 9`, and a list of numbers after the letters (`FCU 1,2,3,&5`).
            for next in words.iter().skip(i + 1).take(12).take_while(|n| n.starts_with(|c: char| c.is_ascii_digit()) || n.as_str() == "AND") {
                if let Some(c) = canon(&format!("{w}-{next}")) {
                    out.insert(c);
                }
            }
        }
    }
    out
}

/// The tags a page's own label names (`Tag: QF-5`, `Mark: RV-1, 2`, `Unit Tags: …`); none when
/// no labelled line names one (a blank `Tag #:`).
fn labelled_tags(page: &str, not_tag: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for line in page.lines().take(MAX_LINES) {
        let up = line.to_ascii_uppercase();
        let at = ["TAGS", "TAG", "MARKS", "MARK"].iter().find_map(|w| {
            up.match_indices(w)
                .find(|(i, _)| {
                    let before = i.checked_sub(1).and_then(|p| up.as_bytes().get(p)).is_none_or(|c| !c.is_ascii_alphanumeric());
                    let after = up.as_bytes().get(i + w.len()).is_none_or(|c| matches!(c, b':' | b'#' | b' ' | b'.'));
                    before && after
                })
                .map(|(i, _)| i + w.len())
        });
        if let Some(rest) = at.and_then(|i| up.get(i..)) {
            out.extend(tags_named(rest, not_tag));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A unit the submittals are read for: its tag (as [`canon`] gives it), the words its kind,
/// title and schedule row give, and its schedule's model numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Unit {
    pub tag: String,
    pub words: BTreeSet<String>,
    pub models: BTreeSet<String>,
    /// What a submittal file's name may call it (its kind's words in the rules data).
    pub file_words: BTreeSet<String>,
    /// The units its schedule row pairs it with (a heat pump and the fan coil it serves).
    pub partners: BTreeSet<String>,
    /// Its section of a split system, when it is one: `Some(true)` outdoor, `Some(false)` indoor.
    pub outdoor: Option<bool>,
}

/// A word without its plural `S`, for matching names (`FANS` → `FAN`).
fn singular(w: &str) -> String {
    if w.len() > 3 && w.ends_with('S') && !w.ends_with("SS") { w.get(..w.len() - 1).unwrap_or(w).to_string() } else { w.to_string() }
}

fn alpha_words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| w.len() >= 3).map(|w| singular(&w.to_ascii_uppercase()))
}

/// The units the spec's equipment are, by tag (supports aside).
pub(crate) fn units(eq: &[MepEquipment], vocab: &Vocab) -> Vec<Unit> {
    let mut by: BTreeMap<String, Unit> = BTreeMap::new();
    for e in eq.iter().take(MAX_UNITS).filter(|e| !support(e)) {
        let Some(tag) = canon(&e.tag) else { continue };
        let u = by.entry(tag.clone()).or_insert_with(|| Unit { tag, ..Unit::default() });
        for w in vocab.kind_words.get(&e.kind.to_ascii_lowercase()).into_iter().flatten() {
            if u.file_words.len() < 50 {
                u.file_words.insert(spaced(w));
            }
        }
        let mut text = format!("{} {}", e.kind.replace('_', " "), e.title);
        for (k, v) in e.properties.iter().take(64) {
            let key = k.to_ascii_uppercase();
            if key.contains("MODEL") {
                let m: String = v.to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
                if m.len() >= 4 && m.len() <= 40 && m.chars().any(|c| c.is_ascii_digit()) && m.chars().any(|c| c.is_ascii_alphabetic()) {
                    u.models.insert(m);
                }
                continue;
            }
            if ["PAIRED", "SERVES", "SERVICE"].contains(&key.as_str()) {
                if let Some(t) = canon(v) {
                    u.partners.insert(t);
                }
                continue;
            }
            if ["STATUS", "BASIS", "SUPPORT"].contains(&key.as_str()) {
                continue;
            }
            text.push(' ');
            text.push_str(v);
        }
        for w in alpha_words(&text) {
            if !vocab.generic.contains(&w) && u.words.len() < 40 {
                u.words.insert(w);
            }
        }
        // Its side: on a roof support (outdoors), else its kind's, else a paired unit's other one.
        let kind = e.kind.to_ascii_lowercase();
        u.outdoor = if e.properties.contains_key("SUPPORT") || vocab.outdoor_kinds.contains(&kind) {
            Some(true)
        } else if vocab.indoor_kinds.contains(&kind) || (!u.partners.is_empty() && kind.contains("split")) {
            Some(false)
        } else {
            u.outdoor
        };
    }
    // A pairing either way round.
    let pairs: Vec<(String, String)> = by.values().flat_map(|u| u.partners.iter().map(move |p| (p.clone(), u.tag.clone()))).collect();
    for (a, b) in pairs {
        if let Some(u) = by.get_mut(&a) {
            u.partners.insert(b);
        }
    }
    by.into_values().collect()
}

/// A roof unit's curb or stand (built with its unit, sized with it).
fn support(e: &MepEquipment) -> bool {
    matches!(e.kind.as_str(), "roof_curb" | "equipment_stand")
}

/// What an approved submittal gives one unit.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Submitted {
    /// Height, width (the long plan side), depth, inches (none: it gives only the curb).
    pub size_in: Option<[f64; 3]>,
    pub curb: Option<Curb>,
    /// `submittal <file> p.<n> (<stamp>)`.
    pub source: String,
}

/// Whether a stamp sends the submittal back.
fn sent_back(status: &str) -> bool {
    let s = status.to_ascii_lowercase();
    s.contains("resubmit") || s.contains("rejected")
}

/// Whether `ctx` holds `w` as a word (or words) of its own.
fn has_word(ctx: &str, w: &str) -> bool {
    ctx.match_indices(w).any(|(i, _)| {
        let before = i.checked_sub(1).and_then(|p| ctx.as_bytes().get(p)).is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = ctx.as_bytes().get(i + w.len()).is_none_or(|c| !c.is_ascii_alphanumeric());
        before && after
    })
}

/// A size's standing for a unit: its tag next to it, the unit's words around it; none for an
/// accessory's size near none of them.
fn score(d: &Dims, u: &Unit, known: &BTreeSet<&str>, heading: &str, vocab: &Vocab) -> Option<i64> {
    // Its tag alone next to it (a line naming two units is both's heading).
    let named: Vec<String> = tags_named(&d.context, &vocab.not_tag).into_iter().filter(|t| known.contains(t.as_str())).collect();
    let tag_hit = named.len() == 1 && named.contains(&u.tag);
    let ctx_words: BTreeSet<String> = alpha_words(&d.context).collect();
    let word_hits = u.words.iter().filter(|w| ctx_words.contains(*w)).count().min(5) as i64;
    let accessory = vocab.accessory.iter().any(|a| has_word(&d.context, a));
    if accessory && !tag_hit && word_hits == 0 {
        return None;
    }
    // A split system's section: the side the words around the size name, else its page's heading;
    // the other side's size is not this unit's.
    let mut side = 0;
    if let Some(outdoor) = u.outdoor {
        let names = |text: &str, words: &[String]| words.iter().any(|w| has_word(text, w));
        let (mine, theirs) = if outdoor { (&vocab.outdoor_words, &vocab.indoor_words) } else { (&vocab.indoor_words, &vocab.outdoor_words) };
        let at = |text: &str| (names(text, mine), names(text, theirs));
        // The nearest line naming one side only (a two-column heading names both), else the
        // page's heading.
        let (m, t) = d.lead.iter().map(|l| at(l)).find(|(a, b)| a != b).unwrap_or_else(|| at(heading));
        match (m, t) {
            (false, true) => return None,
            (true, false) => side = 3,
            _ => {}
        }
    }
    Some(if tag_hit { 10 } else { 0 } + 2 * word_hits + side)
}

/// What one submittal's text gives each unit it is about (see the module doc). `file`: its name.
/// Nothing from a submittal sent back.
pub(crate) fn from_text(text: &str, file: &str, units: &[Unit], vocab: &Vocab) -> BTreeMap<String, Submitted> {
    let mut out = BTreeMap::new();
    let status = super::equipsub::review_status(text);
    if sent_back(&status) || units.is_empty() {
        return out;
    }
    let known: BTreeSet<&str> = units.iter().map(|u| u.tag.as_str()).collect();
    let models: Vec<(&str, &str)> = units.iter().flat_map(|u| u.models.iter().map(move |m| (m.as_str(), u.tag.as_str()))).collect();
    // (page number, its text): `--- page N of M ---` between pages.
    let mut pages: Vec<(u32, &str)> = Vec::new();
    if text.contains("--- page ") {
        for p in text.split("--- page ").skip(1).take(MAX_PAGES as usize) {
            let n = p.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            pages.push((n, p.split_once('\n').map_or("", |(_, rest)| rest)));
        }
    } else {
        pages.push((1, text));
    }
    // Per unit: (score, volume, size, page, curb of its pages).
    let mut cands: BTreeMap<&str, Vec<(i64, f64, [f64; 3], u32)>> = BTreeMap::new();
    let mut curbs: BTreeMap<&str, (u32, Curb)> = BTreeMap::new();
    let mut prev: BTreeSet<&str> = BTreeSet::new();
    for (n, page) in pages {
        let squeezed: String = page.to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
        // A page whose own label names only other units is theirs (a relief hood's in a fan
        // submittal), not the unit before it.
        if labelled_tags(page, &vocab.not_tag).is_some_and(|l| !l.iter().any(|t| known.contains(t.as_str()))) {
            prev.clear();
            continue;
        }
        let mut tags: BTreeSet<&str> = tags_named(page, &vocab.not_tag).iter().filter_map(|t| known.get(t.as_str()).copied()).collect();
        for (m, t) in &models {
            if squeezed.contains(*m) {
                tags.insert(*t);
            }
        }
        // A unit's partner is on its page too (a split system's two sections on one sheet).
        let partners: Vec<&str> = tags
            .iter()
            .filter_map(|t| units.iter().find(|u| u.tag == *t))
            .flat_map(|u| u.partners.iter().filter_map(|p| known.get(p.as_str()).copied()))
            .collect();
        tags.extend(partners);
        if tags.len() > MAX_PAGE_TAGS {
            prev.clear();
            continue;
        }
        if tags.is_empty() {
            tags = prev.clone();
        } else {
            prev = tags.clone();
        }
        if tags.is_empty() {
            continue;
        }
        let sets = dims_in(page, vocab);
        let curb = curb_in(page, vocab);
        let heading = page.lines().filter(|l| !l.trim().is_empty()).take(HEADING_LINES).collect::<Vec<_>>().join(" \n ").to_ascii_uppercase();
        for t in &tags {
            let Some(u) = units.iter().find(|u| u.tag == *t) else { continue };
            for d in &sets {
                let (Some(sz), Some(sc)) = (d.size_in(), score(d, u, &known, &heading, vocab)) else { continue };
                cands.entry(*t).or_default().push((sc, d.volume(), sz, n));
            }
            if let Some(c) = curb {
                curbs.entry(*t).or_insert((n, c));
            }
        }
    }
    let stamp = if status.is_empty() { String::new() } else { format!(" ({status})") };
    for (t, list) in &cands {
        // The best standing; among equals the largest (an accessory is smaller than its unit).
        let Some(best) = list.iter().max_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(b.3.cmp(&a.3))) else { continue };
        out.insert(
            t.to_string(),
            Submitted { size_in: Some(best.2), curb: curbs.get(t).map(|c| c.1), source: format!("submittal {file} p.{}{stamp}", best.3) },
        );
    }
    // A unit whose submittal gives its curb and no size: the curb.
    for (t, (n, c)) in curbs {
        if !cands.contains_key(t) {
            out.insert(t.to_string(), Submitted { size_in: None, curb: Some(c), source: format!("submittal {file} p.{n}{stamp}") });
        }
    }
    out
}

/// Words as one spaced, singular phrase (`Split Systems` → ` SPLIT SYSTEM `), for matching a
/// phrase within a name.
fn spaced(s: &str) -> String {
    let words: Vec<String> = s.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(singular).collect();
    format!(" {} ", words.join(" "))
}

/// The words of a path's name and its two folders, for picking the files that could be about
/// the units.
fn path_words(path: &str) -> BTreeSet<String> {
    let p = std::path::Path::new(path);
    let mut s = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for f in [p.parent(), p.parent().and_then(|f| f.parent())].into_iter().flatten() {
        if let Some(n) = f.file_name() {
            s.push(' ');
            s.push_str(&n.to_string_lossy());
        }
    }
    s.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 2).map(singular).collect()
}

/// Whether a submittal file could be about one of the units by its name (or its folders'): it
/// shares a word with one's title or row, names one's kind as the rules data's words do
/// (`Split Systems` for a heat pump), or holds one's tag letters (`DOAS`).
fn relevant(path: &str, units: &[Unit]) -> bool {
    let words = path_words(path);
    let named = {
        let p = std::path::Path::new(path);
        let mut s = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for f in [p.parent(), p.parent().and_then(|f| f.parent())].into_iter().flatten() {
            if let Some(n) = f.file_name() {
                s.push(' ');
                s.push_str(&n.to_string_lossy());
            }
        }
        spaced(&s)
    };
    units.iter().any(|u| {
        let letters = u.tag.split('-').next().unwrap_or_default();
        (letters.len() >= 2 && words.contains(&singular(letters)))
            || u.words.iter().any(|w| w.len() >= 4 && words.contains(w))
            || u.file_words.iter().any(|w| named.contains(w.as_str()))
    })
}

/// Whether a submittal's first pages name one of the units (its tag, or its model number).
fn names_a_unit(text: &str, units: &[Unit], vocab: &Vocab) -> bool {
    let named = tags_named(text, &vocab.not_tag);
    let squeezed: String = text.to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
    units.iter().any(|u| named.contains(&u.tag) || u.models.iter().any(|m| squeezed.contains(m.as_str())))
}

/// What reading the submittals gave.
#[derive(Clone, Debug, Default)]
pub(crate) struct Reading {
    pub got: BTreeMap<String, Submitted>,
    pub read: Vec<String>,
    pub sent_back: Vec<String>,
    pub unread: Vec<String>,
}

/// Every PDF under `roots` (folders, any depth to 6), sorted.
pub(crate) fn pdfs_under(roots: &[String]) -> Vec<String> {
    fn walk(p: &std::path::Path, out: &mut Vec<String>, depth: usize) {
        if out.len() >= 5_000 || depth > 6 {
            return;
        }
        if p.is_file() {
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                out.push(p.to_string_lossy().to_string());
            }
            return;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            walk(&e, out, depth + 1);
        }
    }
    let mut out = Vec::new();
    for r in roots {
        let r = r.trim();
        if !r.is_empty() {
            walk(std::path::Path::new(r), &mut out, 0);
        }
    }
    out
}

/// The submittals read for the units: the newest revision first, a unit's size from the first
/// file that gives it one.
pub(crate) fn read(files: &[String], units: &[Unit], vocab: &Vocab) -> Reading {
    let mut r = Reading::default();
    if units.is_empty() {
        return r;
    }
    let mut mine: Vec<&String> = files.iter().take(MAX_LISTED_FILES).collect();
    // The files their names tie to a unit first, then the others; each lot the newest revision first.
    mine.sort_by(|a, b| {
        relevant(b, units).cmp(&relevant(a, units)).then(super::equipsub::revision(b).cmp(&super::equipsub::revision(a))).then(a.cmp(b))
    });
    let mut taken = 0usize;
    for f in mine {
        if taken >= MAX_FILES {
            break;
        }
        let name = std::path::Path::new(f.as_str()).file_name().map_or_else(|| f.to_string(), |n| n.to_string_lossy().to_string());
        let bytes = match crate::readfile::read(f.as_str()) {
            Ok(b) if b.len() > MAX_FILE_BYTES => {
                r.unread.push(format!("{name}: larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)));
                continue;
            }
            Ok(b) => b,
            Err(e) => {
                r.unread.push(format!("{name}: {e}"));
                continue;
            }
        };
        // A file its name doesn't tie to a unit: its first pages must name one.
        let mut text = None;
        if !relevant(f, units) {
            match super::spec_import::pdf_text_pages(&bytes, f, PROBE_PAGES) {
                Ok(t) if names_a_unit(&t, units, vocab) => {
                    let pages: u32 = t.split_whitespace().nth(4).and_then(|n| n.parse().ok()).unwrap_or(u32::MAX);
                    if pages <= PROBE_PAGES {
                        text = Some(Ok(t));
                    }
                }
                _ => continue,
            }
        }
        taken += 1;
        let text = match text.unwrap_or_else(|| super::spec_import::pdf_text_pages(&bytes, f, MAX_PAGES)) {
            Ok(t) => t,
            Err(e) => {
                r.unread.push(format!("{name}: {}", e.to_string().chars().take(160).collect::<String>()));
                continue;
            }
        };
        if sent_back(&super::equipsub::review_status(&text)) {
            r.sent_back.push(name);
            continue;
        }
        for (t, s) in from_text(&text, &name, units, vocab) {
            // A later file gives what an earlier one didn't (the size where it gave only the curb).
            match r.got.get_mut(&t) {
                None => {
                    r.got.insert(t, s);
                }
                Some(have) => {
                    if have.size_in.is_none() && s.size_in.is_some() {
                        have.size_in = s.size_in;
                        have.source = s.source;
                    } else if have.curb.is_none() {
                        have.curb = s.curb;
                    }
                }
            }
        }
        r.read.push(name);
    }
    r
}

/// Where a box's size came from: `submittal`, `drawn` (the plan's outline), `schedule`, `rule`
/// (its kind's usual size), else `other`.
pub(crate) fn size_basis(e: &MepEquipment) -> &'static str {
    let src = |k: &str| e.sources.get(k).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let (size, outline, height) = (src("size"), src("outline"), src("height"));
    if size.contains("submittal") {
        "submittal"
    } else if outline.starts_with("outline:") {
        "drawn"
    } else if outline.contains("schedule's size") || height == "schedule" {
        "schedule"
    } else if outline.contains("usual size") || outline.contains("rule") || height.starts_with("rule") {
        "rule"
    } else {
        "other"
    }
}

/// How many boxes have each [`size_basis`].
pub(crate) fn basis_counts(eq: &[MepEquipment]) -> BTreeMap<&'static str, usize> {
    let mut m: BTreeMap<&'static str, usize> = BTreeMap::new();
    for e in eq.iter().filter(|e| !support(e)) {
        *m.entry(size_basis(e)).or_default() += 1;
    }
    m
}

fn fmt_in(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
}

/// A box's size as text: `37.24 W x 16.14 D x 31.89 H in`.
fn size_text(w: f64, d: f64, h: f64) -> String {
    format!("{} W x {} D x {} H in", fmt_in(w), fmt_in(d), fmt_in(h))
}

/// One unit resized.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Resized {
    pub tag: String,
    pub was: String,
    pub now: String,
    pub source: String,
    pub curb_in: Option<f64>,
}

/// What sizing the boxes did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Applied {
    pub resized: Vec<Resized>,
    /// Boxes already the submittal's size (now said so).
    pub confirmed: usize,
    pub curbs: usize,
    /// The spec's places of every box changed (the units and their supports).
    pub changed: Vec<usize>,
    /// Sizes not taken: far off the box's (another product's on a page naming the tag), with why.
    pub refused: Vec<Resized>,
}

/// Each unit an approved submittal sizes takes that size: its centre stays (a box drawn against
/// a wall keeps its back on it), its long side along the box's long side; a roof unit's curb or
/// stand takes the submittal's curb height (and plan size) and the unit stands on it. The size
/// the box had stays beside the new one (`SIZE_WAS`; a schedule's or a plan's as the design value
/// in `corrections`).
pub(crate) fn apply(eq: &mut [MepEquipment], got: &BTreeMap<String, Submitted>, upf: f64) -> Applied {
    let mut out = Applied::default();
    if got.is_empty() || !upf.is_finite() || upf <= 0.0 {
        return out;
    }
    let inch = upf / 12.0;
    let rule = super::provenance::rule(RULE);
    let max_ratio =
        super::provenance::rule_data(RULE).get("max_size_ratio").and_then(Value::as_f64).filter(|r| r.is_finite() && *r > 1.0).unwrap_or(15.0);
    // (level, tag) of each unit resized → its new plan box and the curb its submittal gives.
    let mut placed: BTreeMap<(String, String), ([f64; 2], [f64; 2], Option<Curb>, String)> = BTreeMap::new();
    for (k, e) in eq.iter_mut().enumerate().take(MAX_UNITS) {
        if support(e) {
            continue;
        }
        let Some(sub) = canon(&e.tag).and_then(|t| got.get(&t)) else { continue };
        let Some([h, w, dp]) = sub.size_in else {
            // Only its curb: the unit stays as it is, on that curb.
            out.changed.push(k);
            placed.insert((e.level.to_ascii_lowercase(), e.tag.trim().to_ascii_uppercase()), (e.min, e.max, sub.curb, sub.source.clone()));
            continue;
        };
        let (dx, dy) = ((e.max[0] - e.min[0]).abs(), (e.max[1] - e.min[1]).abs());
        if !(dx.is_finite() && dy.is_finite() && e.height_ft.is_finite()) {
            continue;
        }
        let (wx, wy) = if dx >= dy { (w * inch, dp * inch) } else { (dp * inch, w * inch) };
        let was_in = (dx.max(dy) / inch, dx.min(dy) / inch, e.height_ft * 12.0);
        // A size far off the box's (by volume) is another product's that names the same tag.
        let (was_vol, now_vol) = (was_in.0 * was_in.1 * was_in.2, w * dp * h);
        if was_vol > 0.0 && (now_vol > was_vol * max_ratio || now_vol * max_ratio < was_vol) {
            if out.refused.len() < MAX_LISTED {
                out.refused.push(Resized {
                    tag: e.tag.clone(),
                    was: size_text(was_in.0, was_in.1, was_in.2),
                    now: size_text(w, dp, h),
                    source: format!("{}: more than {max_ratio} times off the box's volume, not taken", sub.source),
                    curb_in: None,
                });
            }
            continue;
        }
        let basis = size_basis(e);
        let same = (was_in.0 - w).abs() <= SAME_IN && (was_in.1 - dp).abs() <= SAME_IN && (was_in.2 - h).abs() <= SAME_IN;
        let was_text = size_text(was_in.0, was_in.1, was_in.2);
        let now_text = size_text(w, dp, h);
        if !same {
            let c = [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0];
            let (was_min, was_max) = (e.min, e.max);
            e.min = [c[0] - wx / 2.0, c[1] - wy / 2.0];
            e.max = [c[0] + wx / 2.0, c[1] + wy / 2.0];
            match e.sources.get("back").map(String::as_str) {
                Some("x-") => (e.min[0], e.max[0]) = (was_min[0], was_min[0] + wx),
                Some("x+") => (e.min[0], e.max[0]) = (was_max[0] - wx, was_max[0]),
                Some("y-") => (e.min[1], e.max[1]) = (was_min[1], was_min[1] + wy),
                Some("y+") => (e.min[1], e.max[1]) = (was_max[1] - wy, was_max[1]),
                _ => {}
            }
            e.height_ft = h / 12.0;
            // The schedule's or the plan's size is the design value beside the field's.
            if matches!(basis, "schedule" | "drawn") {
                let design = format!("{was_text} ({})", if basis == "drawn" { "the plan's outline" } else { "the schedule's size" });
                let corr = RunCorrection {
                    key: "size".into(),
                    design,
                    corrected: now_text.clone(),
                    reason: format!("the approved {} gives the unit's size", sub.source),
                    rule: rule.basis_for(""),
                };
                match e.corrections.iter_mut().find(|c| c.key == "size") {
                    Some(c) => *c = corr,
                    None => e.corrections.push(corr),
                }
            }
            if let Some(old) = e.properties.get("SIZE").cloned() {
                e.properties.entry("SIZE_WAS".into()).or_insert(old);
            } else {
                e.properties.entry("SIZE_WAS".into()).or_insert_with(|| format!("{was_text} ({basis})"));
            }
        }
        e.properties.insert("BOX".into(), format!("{} H x {} W x {} D in", fmt_in(h), fmt_in(w), fmt_in(dp)));
        e.properties.insert("SIZE".into(), now_text.clone());
        e.properties.insert("SIZE_BASIS".into(), sub.source.clone());
        e.sources.insert("size".into(), sub.source.clone());
        e.sources.insert("height".into(), sub.source.clone());
        let at = if basis == "drawn" { "the plan's outline" } else { "where it was placed" };
        e.sources.insert("outline".into(), format!("the approved {}: its size at the centre of {at}", sub.source));
        if e.properties.get("STATUS").map(String::as_str) == Some("inferred") {
            e.properties.insert("STATUS".into(), "derived".into());
        }
        if e.properties.get("STATUS").map(String::as_str) != Some("drawn") {
            e.properties.insert("BASIS".into(), rule.basis_for(&format!("its size from the {}", sub.source)));
        }
        e.confidence = e.confidence.max(0.8);
        out.changed.push(k);
        placed.insert((e.level.to_ascii_lowercase(), e.tag.trim().to_ascii_uppercase()), (e.min, e.max, sub.curb, sub.source.clone()));
        if same {
            out.confirmed += 1;
        } else if out.resized.len() < MAX_UNITS {
            out.resized.push(Resized {
                tag: e.tag.clone(),
                was: was_text,
                now: now_text,
                source: sub.source.clone(),
                curb_in: sub.curb.map(|c| c.height_in),
            });
        }
    }
    // The supports under the units resized: under the new box, at the submittal's curb height.
    let mut curb_top: BTreeMap<(String, String), f64> = BTreeMap::new();
    for (k, s) in eq.iter_mut().enumerate().take(MAX_UNITS) {
        if !support(s) {
            continue;
        }
        let unit = s.properties.get("UNIT").cloned().unwrap_or_else(|| s.tag.rsplit_once(' ').map_or(String::new(), |(t, _)| t.to_string()));
        let key = (s.level.to_ascii_lowercase(), unit.trim().to_ascii_uppercase());
        let Some((min, max, curb, source)) = placed.get(&key) else { continue };
        let (mut lo, mut hi) = (*min, *max);
        if let Some([cl, cw]) = curb.and_then(|c| c.plan_in) {
            let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
            let (ux, uy) = ((hi[0] - lo[0]).abs(), (hi[1] - lo[1]).abs());
            let (wx, wy) = if ux >= uy { (cl * inch, cw * inch) } else { (cw * inch, cl * inch) };
            (lo, hi) = ([c[0] - wx / 2.0, c[1] - wy / 2.0], [c[0] + wx / 2.0, c[1] + wy / 2.0]);
        }
        s.min = lo;
        s.max = hi;
        if let Some(c) = curb {
            let was = s.height_ft * 12.0;
            s.height_ft = c.height_in / 12.0;
            s.properties.insert("STATUS".into(), "derived".into());
            s.properties.insert(
                "BASIS".into(),
                rule.basis_for(&format!("its height {} in from the {source} (the rule's was {} in)", fmt_in(c.height_in), fmt_in(was))),
            );
            s.sources.insert("height".into(), source.clone());
            out.curbs += 1;
        }
        curb_top.insert(key, s.bottom_ft + s.height_ft);
        out.changed.push(k);
    }
    // The unit on its support's top.
    for e in eq.iter_mut().take(MAX_UNITS) {
        if support(e) {
            continue;
        }
        let key = (e.level.to_ascii_lowercase(), e.tag.trim().to_ascii_uppercase());
        if let Some(top) = curb_top.get(&key)
            && (e.bottom_ft - top).abs() > 1e-9
        {
            e.bottom_ft = *top;
            if let Some(v) = e.properties.get_mut("SUPPORT") {
                let part = v.rsplit_once(' ').and_then(|(a, _)| a.rsplit_once(' ')).map_or_else(|| v.clone(), |(a, _)| a.to_string());
                *v = format!("{part} {top:.2} ft");
            }
        }
    }
    out.changed.sort_unstable();
    out.changed.dedup();
    out
}

/// The job's step: the units sized by the job's submittals before they are built. A line for the
/// job's progress, and the report.
pub(crate) fn size_spec(sp: &mut BuildingSpec, submittals: &[String]) -> Option<(Applied, Reading)> {
    if submittals.iter().all(|s| s.trim().is_empty()) || sp.mep.equipment.is_empty() {
        return None;
    }
    let vocab = Vocab::load();
    // The mechanical and plumbing equipment (the gear takes its submittal's box in its own step).
    let theirs: Vec<MepEquipment> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| matches!(super::mep::equipment_trade(sp, e), super::elemtrade::MECHANICAL | super::elemtrade::PLUMBING))
        .cloned()
        .collect();
    let units = units(&theirs, &vocab);
    let files = pdfs_under(submittals);
    let reading = read(&files, &units, &vocab);
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let applied = apply(&mut sp.mep.equipment, &reading.got, upf);
    Some((applied, reading))
}

/// The resized units as the report lists them.
fn listed(a: &Applied) -> Vec<Value> {
    a.resized
        .iter()
        .take(MAX_LISTED)
        .map(|r| {
            let mut v = json!({"tag": r.tag, "was": r.was, "now": r.now, "source": r.source});
            if let Some(c) = r.curb_in {
                v["curb_in"] = json!(c);
            }
            v
        })
        .collect()
}

/// The report of a sizing pass, for a job and the command.
pub(crate) fn report(a: &Applied, r: &Reading, before: &BTreeMap<&'static str, usize>, after: &BTreeMap<&'static str, usize>) -> Value {
    json!({
        "files_read": r.read.len(), "sent_back": r.sent_back, "unread": r.unread.iter().take(MAX_LISTED).collect::<Vec<_>>(),
        "units": r.got.len(), "resized": listed(a), "resized_count": a.resized.len(), "confirmed": a.confirmed, "curbs": a.curbs,
        "refused": a.refused.iter().take(MAX_LISTED).map(|x| json!({"tag": x.tag, "was": x.was, "now": x.now, "why": x.source})).collect::<Vec<_>>(),
        "sized_before": before, "sized_after": after,
    })
}

/// The built elements of the boxes changed take their new size in place: each one rebuilt from
/// the spec, then its geometry and build tags put on the element it replaces (same handle; the
/// properties other steps wrote on it kept). How many were updated.
fn resize_built(d: &mut cadaddy_doc::Drawing, sp: &BuildingSpec, changed: &[usize]) -> usize {
    use cadaddy_geom::{Mat3, Vec2};
    let tags: BTreeSet<String> = changed.iter().filter_map(|k| sp.mep.equipment.get(*k)).map(|e| e.tag.trim().to_ascii_uppercase()).collect();
    if tags.is_empty() {
        return 0;
    }
    let tag_of = |e: &cadaddy_doc::Entity| -> Option<String> {
        (super::bimlook::made_tag(e, "kind") == Some("equipment"))
            .then(|| super::bimlook::made_tag(e, "tag").map(|t| t.trim().to_ascii_uppercase()))
            .flatten()
    };
    let centre = |e: &cadaddy_doc::Entity| -> Option<Vec2> {
        let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
        let n = m.vertices.len().max(1) as f64;
        let (sx, sy) = m.vertices.iter().fold((0.0, 0.0), |(x, y), v| (x + v.x, y + v.y));
        Some(Vec2::new(sx / n, sy / n))
    };
    // The elements built for those tags now: (handle, tag, level, centre less its stacking move, the move).
    let mut old: Vec<(cadaddy_doc::Handle, String, String, Vec2, Vec2)> = d
        .model
        .iter()
        .filter_map(|e| {
            let t = tag_of(e).filter(|t| tags.contains(t))?;
            let off = super::bspec::stacked_by(e);
            Some((e.handle, t, super::bspec::made_level(e).unwrap_or_default().to_ascii_lowercase(), centre(e)? - off, off))
        })
        .collect();
    if old.is_empty() {
        return 0;
    }
    let fresh = super::mep::build_equipment(d, sp, Some(&tags));
    let mut updated = 0usize;
    for h in fresh {
        let Some(new) = d.model.get(h).map(|e| (**e).clone()) else { continue };
        let (Some(t), Some(c)) = (tag_of(&new), centre(&new)) else {
            d.remove_entity(h);
            continue;
        };
        let level = super::bspec::made_level(&new).unwrap_or_default().to_ascii_lowercase();
        let best =
            old.iter().enumerate().filter(|(_, o)| o.1 == t && o.2 == level).min_by(|a, b| a.1.3.dist(c).total_cmp(&b.1.3.dist(c))).map(|(i, _)| i);
        d.remove_entity(h);
        let Some(i) = best else { continue };
        let (oh, _, _, _, off) = old.swap_remove(i);
        let mut kind = new.kind.clone();
        if off.len() > 1e-9 {
            kind.transform(&Mat3::translate(off));
        }
        let mut xdata = if off.len() > 1e-9 { super::bspec::with_stacked(&new.common.xdata, off) } else { new.common.xdata.to_vec() };
        let _ = d.modify_entity(oh, |e| {
            // The properties other steps wrote on the element (its room, its circuit) stay.
            let keys: BTreeSet<String> =
                xdata.iter().filter_map(|t| t.value.strip_prefix("prop:").and_then(|p| p.split_once('=')).map(|(k, _)| k.to_string())).collect();
            let kept: Vec<cadaddy_doc::RawTag> = super::bimlook::made_tags(e)
                .into_iter()
                .flatten()
                .filter(|t| t.code == 1000 && t.value.strip_prefix("prop:").and_then(|p| p.split_once('=')).is_some_and(|(k, _)| !keys.contains(k)))
                .cloned()
                .collect();
            let at = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP));
            let end = at.map(|a| xdata.iter().skip(a + 1).position(|t| t.code == 1001).map_or(xdata.len(), |i| a + 1 + i));
            if let Some(end) = end {
                for (n, t) in kept.into_iter().enumerate() {
                    xdata.insert(end + n, t);
                }
            }
            // Other applications' tags on the element stay too.
            let others: Vec<cadaddy_doc::RawTag> = {
                let x = &e.common.xdata;
                match x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)) {
                    Some(a) => {
                        let rest = x.iter().skip(a + 1).position(|t| t.code == 1001).map_or(x.len(), |i| a + 1 + i);
                        x.iter().take(a).chain(x.iter().skip(rest)).cloned().collect()
                    }
                    None => x.to_vec(),
                }
            };
            xdata.extend(others);
            e.kind = kind;
            e.common.xdata = std::mem::take(&mut xdata).into();
        });
        updated += 1;
    }
    updated
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let mut given: Vec<String> = Vec::new();
    match p.get("paths") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for v in a.iter().take(MAX_FILES) {
                let Some(path) = v.as_str() else { return Err(bad(CMD, format!("`paths` holds paths (strings), not {v}"))) };
                given.push(path.to_string());
            }
        }
        Some(v) => return Err(bad(CMD, format!("`paths` is a list of paths ([\"a.pdf\", \"folder\"]), not {v}"))),
    }
    let dry_run = match p.get("dry_run") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(v) => return Err(bad(CMD, format!("`dry_run` is true or false, not {v}"))),
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let missing: Vec<&str> = given.iter().map(|g| g.trim()).filter(|g| !g.is_empty() && !std::path::Path::new(g).exists()).collect();
        if !missing.is_empty() {
            return Err(bad(CMD, format!("not found: {}", missing.join(", "))));
        }
    }
    if given.is_empty() {
        given = super::guided::load_job(s).ok().flatten().map(|j| j.submittals).unwrap_or_default();
    }
    if given.iter().all(|g| g.trim().is_empty()) {
        return Err(bad(CMD, "no submittals: give `paths` (submittal PDFs or folders), or run it on a job that has them"));
    }
    let (mut sp, stored) = super::bspec::current(s, CMD)?;
    if !stored || sp.mep.equipment.is_empty() {
        return Err(bad(CMD, "the model has no equipment yet: build it first (model.build, or a job)"));
    }
    let before = basis_counts(&sp.mep.equipment);
    let Some((applied, reading)) = size_spec(&mut sp, &given) else {
        return Err(bad(CMD, "nothing to size: no submittals or no equipment"));
    };
    let after = basis_counts(&sp.mep.equipment);
    let mut out = report(&applied, &reading, &before, &after);
    out["dry_run"] = json!(dry_run);
    if dry_run || applied.changed.is_empty() {
        out["elements_updated"] = json!(0);
        return Ok(out);
    }
    let d = s.doc_mut()?;
    d.building_spec = Some(sp.to_json()).into();
    let updated = resize_built(d, &sp, &applied.changed);
    s.touch();
    out["elements_updated"] = json!(updated);
    s.echo(format!(
        "Equipment sizes: {} resized and {} confirmed by their approved submittals ({} curbs); {updated} elements updated",
        applied.resized.len(),
        applied.confirmed,
        applied.curbs
    ));
    Ok(out)
}

#[cfg(test)]
#[path = "equipsize_tests.rs"]
mod tests;
