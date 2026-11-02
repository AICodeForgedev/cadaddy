//! Tamper-resistant receptacles (Q6142): the built-in rule E-RECEPT-TR, run by the prescreen
//! with no agent. Where the rule's code scope (`validate_rules.json`, NEC 406.12) covers a room —
//! a dwelling unit, guest room, guest suite or the like by its name, a typical unit type, or a
//! room name the project adds with `validate.E-RECEPT-TR.rooms` — every 15/20 A, 125 V
//! receptacle (GFCI, duplex, USB, quad) must be tamper-resistant.
//!
//! The tamper-resistance (TR) fact of a receptacle comes from, first that says:
//! 1. its product record (`product.resolved`: a `tamper_resistant` flag or its `features`);
//! 2. its own words: a `tamper_resistant` property, then its type tag (mark, tag, type name or
//!    description: `GFCI-TR`, `TAMPER RESISTANT DUPLEX`), then its own spec device tag, then the
//!    job's legend row or schedule row of its mark;
//! 3. a blanket general note or spec line ("all receptacles shall be tamper-resistant"; a note
//!    set one TEXT per line is read with its other lines).
//!
//! What the job inferred itself (a spec tag field of source `rule` or `default`: the job's own
//! NEC 406.12 take-off) is no document's fact: an element it marked TR that way gets its fact
//! from its plan tag and the documents only.
//!
//! Known not TR is a `fail`; no fact is `unsure` ("tamper-resistance not stated in the model or
//! documents"), never a pass.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::{Model, Prints};
use serde_json::{Value, json};

use super::validate_kind::Kinds;
use super::validate_packet::Area;
use super::validate_prescreen::UnitOf;
use super::validate_rules::{CodeScope, Effective};

/// The rule's id.
pub const RULE: &str = "E-RECEPT-TR";
/// What an unsure verdict says.
pub const NOT_STATED: &str = "tamper-resistance not stated in the model or documents";
/// Objects looked at for the notes, at most (every one).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Characters of one text read, blanket notes kept, and characters of a note cited.
const MAX_TEXT: usize = 20_000;
const MAX_NOTES: usize = 20;
const MAX_CITE: usize = 160;
/// Words of one text looked at, at most.
const MAX_WORDS: usize = 4_000;

/// Lower-case words (letters and digits).
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(MAX_WORDS).map(str::to_ascii_lowercase).collect()
}

/// What a text says of tamper-resistance: `TAMPER-RESISTANT`, `TAMPER RESISTANT`, `TAMPERPROOF`
/// (and, when `bare` — a type tag or legend row — the `TR`/`WRTR` subscript) is yes; the same
/// after `NON`, `NOT` or `NO` is no; nothing is none.
pub fn says_tr(text: &str, bare: bool) -> Option<bool> {
    let w = tokens(text);
    let word = |i: usize| w.get(i).map(String::as_str);
    let mut yes = false;
    for (i, t) in w.iter().enumerate() {
        let hit = match t.as_str() {
            "tamper" => matches!(word(i + 1), Some("resistant" | "resistance" | "resist" | "proof")),
            "tamperproof" | "tamperresistant" => true,
            "nontr" | "nontamper" => return Some(false),
            "tr" | "wrtr" | "trwr" => bare,
            _ => false,
        };
        if hit {
            if i > 0 && matches!(word(i - 1), Some("non" | "not" | "no")) {
                return Some(false);
            }
            yes = true;
        }
    }
    yes.then_some(true)
}

/// A receptacle's tamper-resistance fact and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct TrFact {
    /// Yes, no, or not stated.
    pub tr: Option<bool>,
    /// `product`, `property`, `type_tag`, `legend`, `schedule`, `spec_tag`, `note` or `none`.
    pub from: &'static str,
    /// What says so, cited.
    pub source: String,
}

impl TrFact {
    fn none() -> TrFact {
        TrFact { tr: None, from: "none", source: NOT_STATED.into() }
    }

    pub fn to_json(&self) -> Value {
        json!({"tr": self.tr, "from": self.from, "source": self.source})
    }
}

/// `text` cut for a citation.
fn cite(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > MAX_CITE { format!("{}…", t.chars().take(MAX_CITE).collect::<String>()) } else { t }
}

/// A yes/no a property's text says (`true`, `yes`, `TR`; `false`, `no`, `none`).
fn flag(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" | "tr" => Some(true),
        "false" | "no" | "n" | "0" | "none" => Some(false),
        other => says_tr(other, true),
    }
}

// ---------------------------------------------------------------- the product record

/// A product record (Q6136) as stored on an element: a JSON object, or its text.
fn product_of(e: Option<&Entity>, fields: Option<&BTreeMap<String, String>>) -> Option<Value> {
    let from_prop = e.and_then(|e| {
        let props = cadaddy_doc::props::read(&e.common.xdata);
        props.get("product").filter(|p| p.has_value()).map(|p| p.value.clone())
    });
    let from_field = fields.and_then(|f| f.iter().find(|(k, _)| k.eq_ignore_ascii_case("product")).map(|(_, v)| Value::String(v.clone())));
    let v = from_prop.or(from_field)?;
    match v {
        Value::Object(_) => Some(v),
        Value::String(s) if s.len() <= 100_000 => serde_json::from_str::<Value>(&s).ok().filter(Value::is_object),
        _ => None,
    }
}

/// What a product record's resolved product says of tamper-resistance: its `tamper_resistant`
/// flag, else its features (a list that names TR is yes; one that names features but not TR is
/// no: the product resolved is not a TR one).
fn product_fact(product: &Value) -> Option<TrFact> {
    let resolved = product.get("resolved").filter(|r| r.is_object())?;
    let src = |field: &str| {
        let s = ["sources", "source"].iter().find_map(|k| resolved.get(*k).and_then(|x| x.get(field)).and_then(Value::as_str));
        let s = s.or_else(|| resolved.get("source").and_then(Value::as_str));
        format!("product record{}", s.map(|s| format!(" ({})", cite(s))).unwrap_or_default())
    };
    let explicit = resolved.get("tamper_resistant").and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => flag(s),
        _ => None,
    });
    if let Some(b) = explicit {
        return Some(TrFact { tr: Some(b), from: "product", source: format!("{}: tamper_resistant {b}", src("tamper_resistant")) });
    }
    let features: Vec<String> = match resolved.get("features")? {
        Value::Array(a) => a.iter().take(100).filter_map(Value::as_str).map(|s| s.trim().chars().take(100).collect()).collect(),
        Value::String(s) => s.split([',', ';']).take(100).map(|x| x.trim().chars().take(100).collect()).collect(),
        _ => return None,
    };
    let features: Vec<String> = features.into_iter().filter(|f: &String| !f.is_empty()).collect();
    if features.is_empty() {
        return None;
    }
    let said: Vec<Option<bool>> = features.iter().map(|f| says_tr(f, true)).collect();
    let tr = if said.contains(&Some(false)) { false } else { said.contains(&Some(true)) };
    let list = cite(&features.join(", "));
    let source =
        if tr { format!("{}: features {list}", src("features")) } else { format!("{}: features {list} (no tamper-resistance)", src("features")) };
    Some(TrFact { tr: Some(tr), from: "product", source })
}

// ---------------------------------------------------------------- the blanket notes

/// A general note or spec line that makes every receptacle tamper-resistant.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    /// `general note on E-001`, `spec 26 27 26`.
    pub source: String,
}

/// A text's words without MTEXT format codes (`\P` a new line, `{\fArial;…}` dropped).
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut it = text.chars().take(MAX_TEXT).peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('P' | 'p' | 'X') => out.push('\n'),
                Some('L' | 'l' | 'O' | 'o' | 'K' | 'k' | '~') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some(_) => {
                    // A code with a value: up to its `;`.
                    for x in it.by_ref().take(200) {
                        if x == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            '{' | '}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The sentences of a note text that make receptacles tamper-resistant across the board: TR
/// said (not denied), receptacles named, and a requirement word (all, every, shall…).
fn blanket_sentences(text: &str) -> Vec<String> {
    let t = plain(text);
    let mut out = Vec::new();
    for part in t.split(['\n', ';']).flat_map(|l| l.split(". ")) {
        let w = tokens(part);
        let has = |xs: &[&str]| w.iter().any(|x| xs.contains(&x.as_str()));
        if says_tr(part, false) == Some(true)
            && has(&["receptacle", "receptacles", "outlet", "outlets", "devices", "recepts"])
            && has(&["all", "every", "shall", "must", "provide", "required"])
            && !has(&["not", "non"])
        {
            out.push(cite(part));
        }
    }
    out
}

/// The sheet a model point lies on, by the building spec's sheet rectangles.
fn sheet_at(sp: &BuildingSpec, x: f64, y: f64) -> Option<&str> {
    sp.sheets
        .iter()
        .take(10_000)
        .find(|l| l.rect.is_some_and(|r| x >= r[0].min(r[2]) && x <= r[0].max(r[2]) && y >= r[1].min(r[3]) && y <= r[1].max(r[3])))
        .map(|l| l.number.as_str())
}

/// One TEXT line: where it starts, its height, its words.
struct TextLine<'t> {
    x: f64,
    y: f64,
    h: f64,
    value: &'t str,
}

/// Does a line start a numbered note (`5.`, `12)`)?
fn starts_note(s: &str) -> bool {
    let t = s.trim_start();
    let n = t.chars().take_while(char::is_ascii_digit).count();
    n > 0 && n <= 3 && t.chars().nth(n).is_some_and(|c| c == '.' || c == ')')
}

/// A TEXT line with the lines of its note round it (a note set as one TEXT per line): up to 3
/// lines above and 2 below, each about a line apart and starting near it, until a line that ends
/// a sentence or starts the next numbered note.
fn paragraph(lines: &[TextLine], i: usize) -> String {
    let next = |from: &TextLine, up: bool| {
        lines
            .iter()
            .filter(|l| {
                let dy = if up { l.y - from.y } else { from.y - l.y };
                let h = from.h.max(l.h);
                dy > 0.8 * h && dy <= 2.5 * h && (l.x - from.x).abs() <= 3.0 * h
            })
            .min_by(|a, b| (a.y - from.y).abs().total_cmp(&(b.y - from.y).abs()))
    };
    let Some(me) = lines.get(i) else { return String::new() };
    let mut text: Vec<&str> = vec![me.value];
    let mut cur = me;
    for _ in 0..3 {
        if starts_note(cur.value) {
            break;
        }
        let Some(above) = next(cur, true) else { break };
        if above.value.trim_end().ends_with('.') {
            break;
        }
        text.insert(0, above.value);
        cur = above;
    }
    let mut cur = me;
    for _ in 0..2 {
        if cur.value.trim_end().ends_with('.') {
            break;
        }
        let Some(below) = next(cur, false) else { break };
        if starts_note(below.value) {
            break;
        }
        text.push(below.value);
        cur = below;
    }
    text.iter().map(|t| t.trim()).collect::<Vec<_>>().join(" ")
}

/// The blanket notes of a job: its notes' texts in the drawing, and the prints' spec lines.
pub fn blanket_notes(d: &Drawing, sp: &BuildingSpec, prints: Option<&Prints>) -> Vec<Note> {
    let mut out: Vec<Note> = Vec::new();
    for r in prints.or(sp.prints.as_deref()).map_or(&[][..], |p| p.rules.spec.as_slice()).iter().take(10_000) {
        let applies = r.kind.as_deref().is_none_or(|k| tokens(k).iter().any(|w| w.starts_with("recept") || w == "device" || w == "devices"));
        let said = format!("{} {} {}", r.field, r.allowed.join(" "), r.text);
        if applies && says_tr(&said, false) == Some(true) && out.len() < MAX_NOTES {
            let text = if r.text.trim().is_empty() { said } else { r.text.clone() };
            out.push(Note { text: cite(&text), source: format!("spec {}", r.section.chars().take(40).collect::<String>()) });
        }
    }
    // Cheap first: most texts say nothing of tampering.
    let tamper = |v: &str| v.get(..v.len().min(MAX_TEXT)).unwrap_or(v).to_ascii_lowercase().contains("tamper");
    let mut lines: Option<Vec<TextLine>> = None;
    for e in d.model.iter().take(MAX_ENTITIES) {
        if out.len() >= MAX_NOTES {
            break;
        }
        let (value, at) = match &e.kind {
            EntityKind::Text(t) if tamper(&t.value) => {
                // A TEXT line: with its note's other lines (read once, when first needed).
                let all = lines.get_or_insert_with(|| {
                    d.model
                        .iter()
                        .take(MAX_ENTITIES)
                        .filter_map(|x| match &x.kind {
                            EntityKind::Text(t) if t.height.is_finite() && t.height > 0.0 && t.insert.x.is_finite() && t.insert.y.is_finite() => {
                                Some(TextLine { x: t.insert.x, y: t.insert.y, h: t.height, value: t.value.as_str() })
                            }
                            _ => None,
                        })
                        .collect()
                });
                let me = all.iter().position(|l| l.value == t.value && l.x == t.insert.x && l.y == t.insert.y);
                (me.map_or_else(|| t.value.clone(), |i| paragraph(all, i)), t.insert)
            }
            EntityKind::MText(t) if tamper(&t.contents) => (t.contents.clone(), t.insert),
            _ => continue,
        };
        for text in blanket_sentences(&value) {
            let source = match sheet_at(sp, at.x, at.y) {
                Some(n) => format!("general note on {n} (text {})", e.handle.hex()),
                None => format!("general note (text {})", e.handle.hex()),
            };
            if out.len() < MAX_NOTES && !out.iter().any(|n| n.text == text) {
                out.push(Note { text, source });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- one receptacle

/// What an element is, for its TR fact.
pub struct Subject<'a> {
    pub mark: Option<&'a str>,
    pub tag: Option<&'a str>,
    /// Its name or description.
    pub name: Option<&'a str>,
    /// Its model fields (its type's and the ones a review set).
    pub fields: Option<&'a BTreeMap<String, String>>,
    pub entity: Option<&'a Entity>,
}

/// What the job's documents say of receptacles' tamper-resistance: its tables and its notes.
pub struct Facts<'a> {
    kinds: &'a Kinds,
    d: &'a Drawing,
    pub notes: Vec<Note>,
}

impl<'a> Facts<'a> {
    pub fn of(d: &'a Drawing, sp: &BuildingSpec, kinds: &'a Kinds, prints: Option<&Prints>) -> Facts<'a> {
        Facts { kinds, d, notes: blanket_notes(d, sp, prints) }
    }

    /// The TR fact of one receptacle (see the module's order).
    pub fn fact(&self, s: &Subject) -> TrFact {
        let props = s.entity.map(|e| cadaddy_doc::props::read(&e.common.xdata));
        // 1. Its product record.
        if let Some(f) = product_of(s.entity, s.fields).as_ref().and_then(product_fact) {
            return f;
        }
        // 2. Its own words: a property, then its type tag, its type's fields, the job's tables.
        const FLAGS: [&str; 3] = ["tamper_resistant", "tamper-resistant", "tamperresistant"];
        let prop = props.as_ref().and_then(|p| FLAGS.iter().find_map(|k| p.get(k).filter(|x| x.has_value()).map(|x| x.text())));
        let field = s.fields.and_then(|f| f.iter().find(|(k, _)| FLAGS.iter().any(|x| k.eq_ignore_ascii_case(x))).map(|(_, v)| v.clone()));
        if let Some((text, b)) = prop.or(field).and_then(|t| flag(&t).map(|b| (t, b))) {
            return TrFact { tr: Some(b), from: "property", source: format!("its tamper_resistant property ({})", cite(&text)) };
        }
        // What the job assumed from a rule (NEC 406.12 itself, a default) is no document's fact:
        // its words then stand only in the element's plan tag, not in the type it was given.
        let assumed = self.kinds.assumed_of(s.entity).filter(|a| says_tr(a, true).is_some());
        let mut own: Vec<(String, String)> = Vec::new();
        let name = s.name.filter(|_| assumed.is_none());
        for (what, v) in [("mark", s.mark), ("tag", s.tag), ("name", name)] {
            if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                own.push((what.into(), v.to_string()));
            }
        }
        if let Some(p) = props.as_ref().filter(|_| assumed.is_none()) {
            for k in ["type", "type_name", "description", "name", "legend"] {
                if let Some(x) = p.get(k).filter(|x| x.has_value()) {
                    own.push((k.into(), x.text()));
                }
            }
        }
        for (k, v) in s.fields.into_iter().flatten().filter(|_| assumed.is_none()).take(100) {
            if ["description", "name", "type", "type_name", "notes", "features", "catalog_number", "catalog"]
                .iter()
                .any(|x| k.eq_ignore_ascii_case(x))
            {
                own.push((format!("type {k}"), v.clone()));
            }
        }
        if let Some(t) = s.mark.and_then(|m| self.d.bim_types.types.iter().take(100_000).find(|t| t.mark.trim().eq_ignore_ascii_case(m.trim()))) {
            for (k, f) in t.fields.iter().take(100) {
                own.push((format!("type {} {k}", t.mark), f.value.clone()));
            }
        }
        if let Some(f) = first_said(own.iter().map(|(what, text)| ("type_tag", format!("its {what}"), text.as_str()))) {
            return f;
        }
        let tables = self.kinds.said_of(s.mark, s.entity);
        if let Some(f) = first_said(tables.iter().map(|x| (x.from, x.source.clone(), x.text.as_str()))) {
            return f;
        }
        // 3. A blanket note.
        if let Some(n) = self.notes.first() {
            return TrFact { tr: Some(true), from: "note", source: format!("{}: \"{}\"", n.source, n.text) };
        }
        match assumed {
            Some(a) => TrFact { tr: None, from: "none", source: format!("{NOT_STATED} (the job assumed it, not from a document: \"{}\")", cite(a)) },
            None => TrFact::none(),
        }
    }
}

/// The first of `(from, where, text)` whose text says TR yes or no (a no anywhere among the
/// texts of one source wins over its yes).
fn first_said<'x>(texts: impl Iterator<Item = (&'static str, String, &'x str)>) -> Option<TrFact> {
    let mut yes: Option<TrFact> = None;
    for (from, place, text) in texts {
        match says_tr(text, true) {
            Some(false) => return Some(TrFact { tr: Some(false), from, source: format!("{place}: \"{}\"", cite(text)) }),
            Some(true) if yes.is_none() => yes = Some(TrFact { tr: Some(true), from, source: format!("{place}: \"{}\"", cite(text)) }),
            _ => {}
        }
    }
    yes
}

// ---------------------------------------------------------------- where the code holds

/// Lower case, words joined by `_`.
fn words(s: &str) -> String {
    tokens(s).join("_")
}

fn has_words(hay: &str, needle: &str) -> bool {
    let n = words(needle);
    !n.is_empty() && format!("_{}_", words(hay)).contains(&format!("_{n}_"))
}

/// Why the code requires tamper-resistant receptacles in a room, if it does: a room name of its
/// list in the room's name or its unit type, or (`typical_units`) any typical unit type.
pub(crate) fn required_in(code: &CodeScope, area: &Area, unit: Option<&UnitOf>, print_unit: Option<&str>) -> Option<String> {
    let name = area.name.as_deref().unwrap_or_default();
    let unit_words: Vec<&str> = unit.map(|u| vec![u.id.as_str(), u.name.as_str()]).unwrap_or_default();
    for r in code.rooms.iter().take(super::validate_rules::MAX_CODE_ROOMS) {
        if has_words(name, r) {
            return Some(format!("the room's name ({}) is a {r} ({})", cite(name), code.cite));
        }
        if let Some(u) = unit_words.iter().find(|u| has_words(u, r)) {
            return Some(format!("its unit type ({}) is a {r} ({})", cite(u), code.cite));
        }
    }
    if code.typical_units {
        if let Some(u) = unit {
            return Some(format!("a typical unit (type {}): a dwelling or guest unit ({})", cite(&u.name), code.cite));
        }
        if let Some(t) = print_unit.filter(|t| !t.trim().is_empty()) {
            return Some(format!("a typical unit (type {}): a dwelling or guest unit ({})", cite(t), code.cite));
        }
    }
    None
}

/// The first number in `text` before one of `units` (`20A`, `125 V`, `20 AMP`).
fn rating(text: &str, units: &[&str]) -> Option<f64> {
    let w = tokens(&text.replace('/', " / "));
    for (i, t) in w.iter().enumerate() {
        let split = t.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(t.len());
        let (num, rest) = t.split_at(split);
        let Ok(n) = num.parse::<f64>() else { continue };
        let unit = if rest.is_empty() { w.get(i + 1).map(String::as_str).unwrap_or_default() } else { rest };
        if units.contains(&unit) && n.is_finite() {
            return Some(n);
        }
    }
    None
}

/// Why a receptacle is outside the code's ratings, when its rating is known and above them
/// (a 30 A or 250 V receptacle); an unknown rating is inside.
pub fn outside_ratings(code: &CodeScope, s: &Subject) -> Option<String> {
    let mut texts: Vec<String> = Vec::new();
    const KEYS: [&str; 7] = ["rating", "amps", "amperage", "volts", "voltage", "nema", "configuration"];
    if let Some(e) = s.entity {
        let p = cadaddy_doc::props::read(&e.common.xdata);
        for k in KEYS {
            if let Some(x) = p.get(k).filter(|x| x.has_value()) {
                texts.push(format!("{k} {}", x.text()));
            }
        }
    }
    for (k, v) in s.fields.into_iter().flatten().take(100) {
        if KEYS.iter().any(|x| k.eq_ignore_ascii_case(x)) {
            texts.push(format!("{k} {v}"));
        }
    }
    let rated = |text: &str| -> (Option<f64>, Option<f64>) {
        let (k, v) = text.split_once(' ').unwrap_or((text, ""));
        let mut amps = rating(v, &["a", "amp", "amps", "ampere", "amperes"]);
        let mut volts = rating(v, &["v", "volt", "volts", "vac"]);
        // A bare number under its key.
        let bare = v.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ' ').trim().parse::<f64>().ok();
        if k.starts_with("amp") {
            amps = amps.or(bare);
        }
        if k.starts_with("volt") {
            volts = volts.or(bare);
        }
        // A NEMA configuration: 5-15/5-20 is 125 V; 6- 250 V; 14- 125/250 V.
        let t = tokens(v);
        if let Some((a, b)) = t.windows(2).find_map(|w| match w {
            [a, b] if ["5", "6", "10", "14"].contains(&a.as_str()) => b.trim_end_matches('r').parse::<f64>().ok().map(|b| (a.clone(), b)),
            _ => None,
        }) && (k == "nema" || k == "configuration" || v.to_ascii_lowercase().contains("nema"))
        {
            amps = amps.or(Some(b));
            volts = volts.or(Some(if a == "5" { 125.0 } else { 250.0 }));
        }
        (amps, volts)
    };
    for t in &texts {
        let (a, v) = rated(t);
        if let (Some(a), Some(max)) = (a, code.max_amps)
            && a > max
        {
            return Some(format!("rated {a} A, above the {max} A {} covers", code.cite));
        }
        if let (Some(v), Some(max)) = (v, code.max_volts)
            && v > max
        {
            return Some(format!("rated {v} V, above the {max} V {} covers", code.cite));
        }
    }
    None
}

// ---------------------------------------------------------------- packets

/// Each receptacle of a packet's `objects.json` elements gets its TR fact and source, and
/// whether (and why) the code wants it tamper-resistant in this area.
#[allow(clippy::too_many_arguments)]
pub(crate) fn annotate(
    d: &Drawing,
    sp: &BuildingSpec,
    model: &Model,
    kinds: &Kinds,
    prints: Option<&Prints>,
    rules: &[Effective],
    area: &Area,
    elements: &mut [Value],
) {
    let Some(r) = rules.iter().find(|r| r.rule.id == RULE) else { return };
    let Some(code) = &r.rule.code else { return };
    let is_receptacle = |e: &Value| {
        let s = |k: &str| e.get(k).and_then(Value::as_str);
        r.rule.applies_to_element(s("kind").unwrap_or_default(), s("category"), s("mark"))
    };
    if !elements.iter().any(is_receptacle) {
        return;
    }
    let facts = Facts::of(d, sp, kinds, prints);
    let unit = super::validate_prescreen::unit_of(d, &area.level, area.number.as_deref());
    let print_unit = prints.or(sp.prints.as_deref()).and_then(|p| {
        p.rooms.iter().take(200_000).find_map(|pr| {
            (pr.level.trim().eq_ignore_ascii_case(area.level.trim())
                && area.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(pr.number.trim())))
            .then(|| pr.unit_type.clone())
            .flatten()
        })
    });
    let why_room = required_in(code, area, unit.as_ref(), print_unit.as_deref());
    for e in elements.iter_mut().filter(|e| is_receptacle(e)) {
        let handle = e.get("handle").and_then(Value::as_str).unwrap_or_default().to_string();
        let el = model.elements.iter().find(|x| x.id.eq_ignore_ascii_case(&handle));
        let ent = cadaddy_doc::Handle::parse_hex(&handle).and_then(|h| d.model.get(h)).map(|x| x.as_ref());
        let text = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        let (mark, tag, name) = (text("mark"), text("tag"), text("name"));
        let subject = Subject { mark: mark.as_deref(), tag: tag.as_deref(), name: name.as_deref(), fields: el.map(|x| &x.fields), entity: ent };
        let fact = facts.fact(&subject);
        let out_of = outside_ratings(code, &subject);
        let mut v = fact.to_json();
        v["required"] = json!(why_room.is_some() && out_of.is_none());
        v["required_because"] = match (&why_room, &out_of) {
            (Some(w), None) => json!(w),
            (Some(_), Some(o)) => json!(format!("not required: {o}")),
            (None, _) => json!(format!("not required: {} does not list this room", code.cite)),
        };
        v["rule"] = json!(RULE);
        e["tamper_resistant"] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_say_tamper_resistant_or_not() {
        for t in ["TAMPER-RESISTANT DUPLEX", "Tamper Resistant", "tamperproof", "GFCI, TAMPER RESISTANCE"] {
            assert_eq!(says_tr(t, false), Some(true), "{t}");
        }
        for t in ["GFCI-TR", "TR", "REC TR", "WRTR"] {
            assert_eq!(says_tr(t, true), Some(true), "{t}");
            assert_eq!(says_tr(t, false), None, "a bare TR is a tag's, not a note's: {t}");
        }
        for t in ["NON-TAMPER RESISTANT", "not tamper-resistant", "NON-TR", "nontr"] {
            assert_eq!(says_tr(t, true), Some(false), "{t}");
        }
        for t in ["GFCI DUPLEX", "TRANSFER SWITCH", "STR", "", "tamper"] {
            assert_eq!(says_tr(t, true), None, "{t}");
        }
    }

    #[test]
    fn a_blanket_note_is_a_requirement_for_every_receptacle() {
        let mt = "{\\fArial|b0;GENERAL NOTES:\\P1. ALL RECEPTACLES SHALL BE TAMPER-RESISTANT.\\P2. SEE SPECS.}";
        assert_eq!(blanket_sentences(mt), vec!["ALL RECEPTACLES SHALL BE TAMPER-RESISTANT."]);
        // A legend row, a denial, a note of something else: no blanket.
        for t in ["TAMPER RESISTANT DUPLEX RECEPTACLE", "RECEPTACLES SHALL NOT BE TAMPER RESISTANT", "ALL PANELS SHALL BE TAMPERPROOF"] {
            assert!(blanket_sentences(t).is_empty(), "{t}");
        }
    }

    #[test]
    fn a_note_set_one_text_per_line_is_read_whole() {
        let l = |y: f64, x: f64, v: &'static str| TextLine { x, y, h: 1.0, value: v };
        let lines = [
            l(10.0, 0.0, "4. SEE THE SPECS."),
            l(8.5, 0.0, "5. ALL GUEST ROOM 125V RECEPTACLES SHALL BE"),
            l(7.0, 1.0, "TAMPER-RESISTANT AS REQUIRED"),
            l(5.5, 1.0, "BY CODE."),
            l(4.0, 0.0, "6. NEXT NOTE"),
            l(7.0, 40.0, "FAR AWAY"),
        ];
        let p = paragraph(&lines, 2);
        assert_eq!(p, "5. ALL GUEST ROOM 125V RECEPTACLES SHALL BE TAMPER-RESISTANT AS REQUIRED BY CODE.");
        assert_eq!(blanket_sentences(&p), vec!["ALL GUEST ROOM 125V RECEPTACLES SHALL BE TAMPER-RESISTANT AS REQUIRED BY CODE."]);
        assert!(starts_note("12) x") && !starts_note("TR") && !starts_note("1234. x"));
    }

    #[test]
    fn a_product_record_says_tr_by_its_flag_or_its_features() {
        let f = |v: Value| product_fact(&v);
        let yes = f(json!({"resolved": {"features": ["duplex", "tamper-resistant"], "sources": {"features": "spec 26 27 26 p. 3"}}})).unwrap();
        assert_eq!((yes.tr, yes.from), (Some(true), "product"));
        assert!(yes.source.contains("26 27 26"), "{yes:?}");
        let no = f(json!({"resolved": {"features": "GFCI, duplex"}})).unwrap();
        assert_eq!(no.tr, Some(false), "a feature list without TR: {no:?}");
        assert_eq!(f(json!({"resolved": {"tamper_resistant": false, "features": ["TR"]}})).unwrap().tr, Some(false), "the flag first");
        assert!(f(json!({"resolved": {"maker": "Acme"}})).is_none());
        assert!(f(json!({"required": {"features": ["TR"]}})).is_none(), "a requirement is not the product");
    }

    /// A built receptacle whose spec tag the job marked TR from the code itself (source `rule`)
    /// is not TR by any document: unsure, saying so. A tag field read off the plan is a fact.
    #[test]
    fn what_the_job_assumed_from_the_code_is_no_fact() {
        use cadaddy_doc::props::{Prop, Source};
        use cadaddy_walk3d::tags::ElementTag;
        let built = |devid: &str| {
            let mut e = Entity::new(
                cadaddy_doc::Handle(0x50),
                EntityKind::Point(cadaddy_doc::Point { p: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0), angle: 0.0 }),
            );
            let tag = |v: &str| cadaddy_doc::RawTag { code: 1000, value: v.into() };
            e.common.xdata =
                vec![cadaddy_doc::RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }, tag("device"), tag("L1"), tag(&format!("devid:{devid}"))]
                    .into();
            let mut store = cadaddy_doc::props::read(&e.common.xdata);
            store.set("type", Prop::new("duplex receptacle, tamper-resistant", Source::of(cadaddy_doc::props::SourceKind::Print)));
            e.common.xdata = cadaddy_doc::props::write(&e.common.xdata, &store).into();
            e
        };
        let mut assumed = ElementTag { id: "device:L1:1,1".into(), kind: "receptacle".into(), level: "L1".into(), ..ElementTag::default() };
        assumed.set("type", "REC", "plan");
        assumed.set("device_type", "duplex receptacle, tamper-resistant", "rule");
        assumed.set("tamper_resistant", "the code: a guest room's receptacles", "rule");
        let mut read = ElementTag { id: "device:L1:2,1".into(), kind: "receptacle".into(), level: "L1".into(), ..ElementTag::default() };
        read.set("type", "REC", "plan");
        read.set("device_type", "duplex receptacle, tamper-resistant", "plan");
        let sp = BuildingSpec { tags: vec![assumed, read], ..BuildingSpec::default() };
        let d = Drawing::default();
        let kinds = Kinds::of(&d, &sp, None);
        let facts = Facts::of(&d, &sp, &kinds, None);
        let fact = |devid: &str| {
            let e = built(devid);
            facts.fact(&Subject { mark: Some("REC"), tag: None, name: Some("duplex receptacle, tamper-resistant"), fields: None, entity: Some(&e) })
        };
        let a = fact("device:L1:1,1");
        assert_eq!(a.tr, None, "{a:?}");
        assert!(a.source.starts_with(NOT_STATED) && a.source.contains("assumed"), "{a:?}");
        let r = fact("device:L1:2,1");
        assert_eq!((r.tr, r.from), (Some(true), "type_tag"), "{r:?}");
    }

    #[test]
    fn ratings_above_the_codes_leave_a_receptacle_out() {
        let code = CodeScope {
            cite: "NEC 406.12".into(),
            says: "x".into(),
            rooms: vec!["guest room".into()],
            typical_units: true,
            max_amps: Some(20.0),
            max_volts: Some(125.0),
        };
        let out = |k: &str, v: &str| {
            let fields: BTreeMap<String, String> = [(k.to_string(), v.to_string())].into();
            outside_ratings(&code, &Subject { mark: None, tag: None, name: None, fields: Some(&fields), entity: None })
        };
        assert!(out("rating", "20A 125V").is_none());
        assert!(out("rating", "15 AMP").is_none());
        assert!(out("rating", "30A 250V").is_some());
        assert!(out("volts", "208").is_some());
        assert!(out("nema", "6-20R").is_some());
        assert!(out("nema", "5-20R").is_none());
        assert!(out("amps", "lots").is_none(), "unknown is inside");
    }

    #[test]
    fn the_rooms_the_code_lists_need_tr() {
        let code = CodeScope {
            cite: "NEC 406.12".into(),
            says: "x".into(),
            rooms: vec!["guest room".into(), "dwelling unit".into()],
            typical_units: true,
            max_amps: None,
            max_volts: None,
        };
        let area = |name: &str| Area {
            id: "L1/1".into(),
            kind: "room",
            level: "L1".into(),
            number: Some("1".into()),
            name: Some(name.into()),
            room: None,
            stretch: None,
            outline: Vec::new(),
            bbox: [0.0; 4],
        };
        assert!(required_in(&code, &area("GUEST ROOM 214"), None, None).is_some_and(|w| w.contains("guest room")));
        assert!(required_in(&code, &area("CORRIDOR"), None, None).is_none());
        assert!(required_in(&code, &area("GUEST LAUNDRY"), None, None).is_none());
        let unit = UnitOf { id: "ut-k".into(), name: "KING".into(), hand: "normal".into(), ty: Value::Null, fit: Value::Null };
        assert!(required_in(&code, &area("UNIT"), Some(&unit), None).is_some_and(|w| w.contains("typical unit")));
        assert!(required_in(&code, &area("UNIT"), None, Some("QQ")).is_some());
        let mut no_units = code.clone();
        no_units.typical_units = false;
        assert!(required_in(&no_units, &area("UNIT"), Some(&unit), None).is_none());
    }
}
